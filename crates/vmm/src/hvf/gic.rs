// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. Firecracker has no userspace interrupt controller
// (its aarch64 port drives the kernel's vGIC through KVM), so there is nothing
// upstream to copy. This is an independent implementation written from the
// published GICv3 architecture specification (Arm IHI 0069) and from the
// register and field set the Linux driver `drivers/irqchip/irq-gic-v3.c` uses.
//
// License gate: QEMU's GICv3 *controller model* (`hw/intc/arm_gicv3*`) is
// GPL-2.0 and was not consulted or copied; the register semantics below come
// from the architecture specification and the kernel's documented register
// use. The `ICC_*` system-register *encodings* (`sysreg(3, 0, 12, ...)` and
// friends at the bottom of this file) were taken from the `SYSREG_ICC_*` table
// in QEMU's `target/arm/helper.c`, as the backend's research notes record:
// those are encoding constants, not code. The controller model, its state and
// its delivery rules are written here.

//! A userspace GICv3 for the Hypervisor.framework backend.
//!
//! Hypervisor.framework provides no in-kernel interrupt controller to a VMM
//! that does not opt into macOS 15's `hv_gic_*` API, so this module emulates
//! one:
//!
//! * the **distributor** (SPIs) and one **redistributor** per vCPU (SGIs and
//!   PPIs) are memory-mapped: the guest's loads and stores to them leave the
//!   guest as data aborts and are dispatched here;
//! * the **CPU interfaces** are the `ICC_*` system registers: HVF traps the
//!   guest's `MRS`/`MSR` to them (exit class `EC_SYSTEMREGISTERTRAP`) and this
//!   module answers, exactly as a KVM VMM would let the in-kernel vGIC answer;
//! * delivery is via [`Gic::irq_line`], which the vCPU thread mirrors into
//!   `hv_vcpu_set_pending_interrupt`.
//!
//! The model is the smallest one that satisfies a serial-console microVM
//! running Linux: 128 interrupts (32 SGIs/PPIs and 96 SPIs, the same range
//! Firecracker's aarch64 port exposes), priorities (5 implemented bits, as the
//! kernels' vGIC uses), Group 1 only, affinity routing through `GICD_IROUTER`,
//! and no LPIs, ITS, GICv4 virtual interrupts or Group 0. Registers it does not
//! implement read as zero and ignore writes (RES0/WI), which keeps the model
//! total in the face of guest probing.
//!
//! This module is deliberately free of any FFI and of any `crate::arch`
//! dependency so it builds and is unit-tested on the host.

/// Number of private interrupts (SGIs 0..16 and PPIs 16..32) per CPU.
pub const NUM_PRIVATE_INTIDS: usize = 32;
/// Total number of interrupts: the 32 private ones plus 96 SPIs.
pub const NUM_INTIDS: usize = 128;
/// First SPI: `GICD_*` and `ICC_IAR*` report an interrupt that is not pending
/// as this value when the CPU interface is asked to acknowledge.
pub const SPURIOUS_INTID: u32 = 1023;
/// The interrupt ID of the first SPI.
pub const SPI_BASE: u32 = NUM_PRIVATE_INTIDS as u32;
/// Number of priority bits this model implements (as the kernels' vGIC does).
pub const NR_PRIORITY_BITS: u32 = 5;
/// Where the priority mask register starts out; the guest raises it as it
/// unmasks interrupts.
pub const RESET_PMR: u8 = 0x00;
/// Priority of an interrupt that is signalled before the guest programs it.
pub const DEFAULT_PRIORITY: u8 = 0xa0;
/// The running priority with no interrupt active (`ICC_RPR_EL1` when nothing
/// is running): the lowest possible priority, so any enabled interrupt that
/// passes the priority mask is signalled.
pub const IDLE_PRIORITY: u8 = 0xff;
/// The maintenance interrupt (PPI), declared by the device tree.
pub const MAINTENANCE_PPI: u32 = 9;
/// The virtual timer interrupt as the device tree's timer node numbers it, and
/// as the vCPU loop's `CNTV_CTL_EL0` bookkeeping refers to it.
///
/// This is *not* the interrupt ID the timer reaches the guest as: see
/// [`VIRTUAL_TIMER_INTID`].
pub const VIRTUAL_TIMER_PPI: u32 = 11;
/// The interrupt ID of the virtual timer (PPI) interrupt: the device tree's
/// PPI number plus the 16 SGIs, which is what the guest enables and
/// acknowledges.
///
/// The device tree declares a PPI by its number, `11` here, and the kernel
/// translates a device tree PPI *n* into hardware interrupt `n + 16`
/// (`drivers/irqchip/irq-gic-v3.c`), so a guest enables bit 27 of
/// `GICR_ISENABLER0` and reads 27 back from `ICC_IAR1_EL1`. Raising the device
/// tree's number instead would pend an SGI the guest never enabled, and a
/// Linux guest - whose tick is the virtual timer under a hypervisor - would
/// never take a timer interrupt. [`Gic::raise_virtual_timer`] and
/// [`Gic::lower_virtual_timer`] are the only things that should touch it.
pub const VIRTUAL_TIMER_INTID: u32 = VIRTUAL_TIMER_PPI + 16;
/// The serial console's SPI, as the device tree's `uart` node numbers it: the
/// first legacy GSI, as on the KVM backend (`arch::aarch64::layout`), so that
/// the virtio transports take the layout's own GSIs (vsock 1, workspace 2,
/// data volumes from 8) on this backend too.
pub const UART_SPI: u32 = 0;
/// The serial console's interrupt ID (`UART_SPI` offset by the SPI base).
pub const UART_INTID: u32 = SPI_BASE + UART_SPI;

/// Base of the distributor's MMIO window.
///
/// 128 MiB and 128 MiB + 640 KiB are the addresses a `virt` machine uses for a
/// GICv3; both are far below this port's 1 GiB MMIO window
/// (`arch::aarch64::layout::MMIO32_MEM_START`) and below the guest's DRAM at
/// 2 GiB, so no guest memory or device window is shadowed. Nothing needs the
/// guest to use these addresses in particular - the device tree tells it where
/// the controller is - but staying on the conventional ones keeps guests that
/// second-guess the device tree working.
pub const GICV3_DIST_BASE: u64 = 0x0800_0000;
/// Size of the distributor's MMIO window (`GIC_V3_DIST_SIZE`).
pub const GICV3_DIST_SIZE: u64 = 0x1_0000;
/// Base of the redistributor region.
pub const GICV3_REDIST_BASE: u64 = 0x080a_0000;
/// Distance between two redistributors: `RD_base` and `SGI_base` frames.
pub const GICV3_REDIST_STRIDE: u64 = 0x2_0000;
/// Offset of the SGI frame from a redistributor's `RD_base`.
pub const GICV3_SGI_BASE_OFFSET: u64 = 0x1_0000;

/// The MMIO window [`GICV3_REDIST_BASE`] covers for `vcpu_count` vCPUs.
pub const fn redist_window(vcpu_count: usize) -> (u64, u64) {
    (GICV3_REDIST_BASE, GICV3_REDIST_STRIDE * vcpu_count as u64)
}

/// Which part of the controller a guest physical address falls in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GicMmio {
    /// A distributor register, at `offset` from [`GICV3_DIST_BASE`].
    Distributor {
        /// The offset from the distributor's base.
        offset: u64,
    },
    /// A redistributor register of `cpu`, at `offset` from that
    /// redistributor's `RD_base`.
    Redistributor {
        /// The vCPU the redistributor belongs to.
        cpu: usize,
        /// The offset from the redistributor's base.
        offset: u64,
    },
}

/// Whether `addr` falls inside `[base, base + len)`.
pub const fn in_window(addr: u64, base: u64, len: u64) -> bool {
    addr >= base && addr - base < len
}

// ---------------------------------------------------------------------------
// Distributor (`GICD_*`) and redistributor (`GICR_*`) register offsets. The
// private registers of a redistributor's SGI frame reuse the distributor's
// offsets, as they do in the architecture.
// ---------------------------------------------------------------------------

const GICD_CTLR: u64 = 0x0000;
const GICD_TYPER: u64 = 0x0004;
const GICD_IIDR: u64 = 0x0008;
const GICD_TYPER2: u64 = 0x000c;
const GICD_STATUSR: u64 = 0x0010;
const GICD_IGROUPR: u64 = 0x0080;
const GICD_ISENABLER: u64 = 0x0100;
const GICD_ICENABLER: u64 = 0x0180;
const GICD_ISPENDR: u64 = 0x0200;
const GICD_ICPENDR: u64 = 0x0280;
const GICD_ISACTIVER: u64 = 0x0300;
const GICD_ICACTIVER: u64 = 0x0380;
const GICD_IPRIORITYR: u64 = 0x0400;
const GICD_ICFGR: u64 = 0x0c00;
const GICD_IROUTER: u64 = 0x6000;
const GICD_PIDR2: u64 = 0xffe8;

const GICR_CTLR: u64 = 0x0000;
const GICR_IIDR: u64 = 0x0004;
const GICR_TYPER: u64 = 0x0008;
const GICR_STATUSR: u64 = 0x0010;
const GICR_WAKER: u64 = 0x0014;
const GICR_PIDR2: u64 = 0xffe8;

/// `GICD_CTLR.EnGrp1` (or `.EnGrp1NS` with security enabled, which this model
/// never is: `DS` is reported as written and no Group 0 exists).
const GICD_CTLR_ENABLE_GRP1: u32 = 1 << 0;
/// `GICD_CTLR.RWP`: reads as zero here (all changes are immediate).
const GICD_CTLR_RWP: u32 = 1 << 31;

/// `GICR_WAKER.ProcessorSleep` / `.ChildrenAsleep`.
const GICR_WAKER_PROCESSOR_SLEEP: u32 = 1 << 1;
const GICR_WAKER_CHILDREN_ASLEEP: u32 = 1 << 2;

/// `GICR_TYPER.Last`: set on the last redistributor of the region.
const GICR_TYPER_LAST: u32 = 1 << 4;

/// `GICD_IROUTER<n>.IRM`: route the interrupt to any CPU.
const GICD_IROUTER_IRM: u32 = 1 << 31;

/// `GIC[D]_PIDR2`: reports the architecture revision in bits 7:4, GICv3 being
/// `0x3`. The kernel refuses the controller unless this reads back as GICv3
/// (or GICv4), and reads it from the distributor as well as from each
/// redistributor.
const GIC_PIDR2_GICV3: u32 = 0x30;

/// State of one interrupt that a single agent owns: a distributor SPI, or a
/// private interrupt of one redistributor.
#[derive(Debug, Clone)]
struct Interrupts<const N: usize> {
    /// `ICFGR`: the 2-bit configuration field. Stored only - this model always
    /// treats a raised interrupt as a latched edge - but the guest programs it,
    /// and reading it back must not invent different bits.
    config: [u8; N],
    /// `IGROUPR`: `true` = Group 1, the only group this model delivers.
    group1: [bool; N],
    /// `ISENABLER`/`ICENABLER`.
    enabled: [bool; N],
    /// `ISPENDR`/`ICPENDR`.
    pending: [bool; N],
    /// `ISACTIVER`/`ICACTIVER`.
    active: [bool; N],
    /// `IPRIORITYR`, one byte per interrupt.
    priority: [u8; N],
}

impl<const N: usize> Interrupts<N> {
    fn new() -> Self {
        Self {
            config: [0; N],
            group1: [false; N],
            enabled: [false; N],
            pending: [false; N],
            active: [false; N],
            priority: [DEFAULT_PRIORITY; N],
        }
    }

    /// Applies a write of `size` bytes at `offset` (relative to `base`) to one
    /// of the bit-per-interrupt registers (`IGROUPR`, `ISENABLER`, ...): each
    /// set bit of `value` names an interrupt.
    fn write_bits(
        &mut self,
        base: u64,
        offset: u64,
        size: u8,
        value: u64,
        bits: impl Fn(&mut Self, usize),
    ) {
        let first = (offset - base) * 8;
        for i in 0..size as u64 * 8 {
            let intid = first + i;
            if intid >= N as u64 {
                break;
            }
            if (value >> i) & 1 == 1 {
                bits(self, intid as usize);
            }
        }
    }

    /// Applies a write to `IPRIORITYR`: one byte per interrupt.
    fn write_priority(&mut self, offset: u64, size: u8, value: u64) {
        for i in 0..size as u64 {
            let intid = offset - GICD_IPRIORITYR + i;
            if intid >= N as u64 {
                break;
            }
            self.priority[intid as usize] = ((value >> (8 * i)) & 0xff) as u8;
        }
    }

    /// Applies a write to `ICFGR`: two bits per interrupt, four interrupts per
    /// byte.
    fn write_config(&mut self, offset: u64, size: u8, value: u64) {
        for i in 0..size as u64 * 4 {
            let intid = (offset - GICD_ICFGR) * 4 + i;
            if intid >= N as u64 {
                break;
            }
            self.config[intid as usize] = ((value >> (2 * i)) & 0x3) as u8;
        }
    }

    /// The `IPRIORITYR` bytes starting at `offset`, packed into a word.
    fn read_priority(&self, offset: u64, size: u8) -> u64 {
        let mut value = 0u64;
        for i in 0..size as u64 {
            let intid = offset - GICD_IPRIORITYR + i;
            if intid >= N as u64 {
                break;
            }
            value |= u64::from(self.priority[intid as usize]) << (8 * i);
        }
        value
    }

    /// The `ICFGR` bits starting at `offset`, packed into a word.
    fn read_config(&self, offset: u64, size: u8) -> u64 {
        let mut value = 0u64;
        for i in 0..size as u64 * 4 {
            let intid = (offset - GICD_ICFGR) * 4 + i;
            if intid >= N as u64 {
                break;
            }
            value |= u64::from(self.config[intid as usize] & 3) << (2 * i);
        }
        value
    }

    /// The bit-per-interrupt registers (`IGROUPR`, `ISENABLER`, `ISPENDR`,
    /// `ISACTIVER`) starting at `offset`, packed into a word.
    fn read_bits(&self, offset: u64, size: u8, bits: impl Fn(&Self, usize) -> bool) -> u64 {
        let first = offset * 8;
        let mut value = 0u64;
        for i in 0..size as u64 * 8 {
            let intid = first + i;
            if intid >= N as u64 {
                break;
            }
            if bits(self, intid as usize) {
                value |= 1 << i;
            }
        }
        value
    }
}

/// One CPU interface: the `ICC_*` system registers of a vCPU, plus the
/// private interrupts its redistributor owns.
#[derive(Debug, Clone)]
struct CpuInterface {
    private: Interrupts<NUM_PRIVATE_INTIDS>,
    /// `ICC_SRE_EL1`: `SRE` plus the two "disable" bits the guest may set.
    sre: u64,
    /// `ICC_PMR_EL1`: interrupts at or below this priority are masked.
    pmr: u8,
    /// `ICC_CTLR_EL1` as written; the read-only fields are or-ed in on read.
    ctlr: u32,
    /// `ICC_BPR0_EL1` / `ICC_BPR1_EL1`, stored (no preemption is modelled).
    bpr0: u32,
    bpr1: u32,
    /// `ICC_IGRPEN0_EL1` / `ICC_IGRPEN1_EL1`.
    igrpen0: bool,
    igrpen1: bool,
    /// `ICC_AP0R<n>_EL1`: stored only, as Group 0 is not modelled.
    ap0r: [u32; 4],
    /// `ICC_AP1R<n>_EL1`. Only `ap1r[0]` is used: bit `g` is set while an
    /// interrupt of priority group `g` (`priority >> 3`) is active. The running
    /// priority is derived from it, so a guest write here takes effect at once.
    ap1r: [u32; 4],
    /// `ICC_RPR_EL1`: the running priority, derived from `ap1r` (see
    /// [`CpuInterface::refresh_running_priority`]); [`IDLE_PRIORITY`] when no
    /// interrupt is active. An interrupt is signalled only if its priority is
    /// higher than this (see [`CpuInterface::accepts`]).
    running_priority: u8,
    /// `GICR_WAKER`.
    waker: u32,
}

impl CpuInterface {
    fn new() -> Self {
        Self {
            private: Interrupts::new(),
            sre: 0,
            pmr: RESET_PMR,
            ctlr: 0,
            bpr0: 0,
            bpr1: 0,
            igrpen0: false,
            igrpen1: false,
            ap0r: [0; 4],
            ap1r: [0; 4],
            running_priority: IDLE_PRIORITY,
            waker: GICR_WAKER_PROCESSOR_SLEEP | GICR_WAKER_CHILDREN_ASLEEP,
        }
    }

    /// The running priority the active interrupts imply: the group of the
    /// highest active priority, which is the lowest set bit of `ap1r[0]`, or
    /// [`IDLE_PRIORITY`] when no bit is set. With [`NR_PRIORITY_BITS`] = 5 a
    /// group holds 8 priorities, so priority `p` is group `p >> 3`.
    fn derived_running_priority(&self) -> u8 {
        if self.ap1r[0] == 0 {
            IDLE_PRIORITY
        } else {
            (self.ap1r[0].trailing_zeros() as u8) << 3
        }
    }

    /// Sets `running_priority` from the active priorities in `ap1r`.
    fn refresh_running_priority(&mut self) {
        self.running_priority = self.derived_running_priority();
    }

    /// Whether the CPU interface would signal an interrupt of `priority`
    /// right now: Group 1 is enabled, the priority passes the mask, and the
    /// priority is higher than the running priority. An active interrupt is
    /// therefore not signalled again before its end-of-interrupt, and only a
    /// higher-priority one preempts it.
    fn accepts(&self, priority: u8) -> bool {
        self.igrpen1 && priority < self.pmr && priority < self.running_priority
    }
}

/// One agent's interrupts as a snapshot keeps them (plain vectors: the model's fixed arrays are
/// longer than serde's derive takes).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InterruptsState {
    pub config: Vec<u8>,
    pub group1: Vec<bool>,
    pub enabled: Vec<bool>,
    pub pending: Vec<bool>,
    pub active: Vec<bool>,
    pub priority: Vec<u8>,
}

impl<const N: usize> Interrupts<N> {
    fn state(&self) -> InterruptsState {
        InterruptsState {
            config: self.config.to_vec(),
            group1: self.group1.to_vec(),
            enabled: self.enabled.to_vec(),
            pending: self.pending.to_vec(),
            active: self.active.to_vec(),
            priority: self.priority.to_vec(),
        }
    }

    fn from_state(state: &InterruptsState) -> Result<Self, String> {
        fn take<T: Copy, const N: usize>(what: &str, from: &[T]) -> Result<[T; N], String> {
            <[T; N]>::try_from(from)
                .map_err(|_| format!("{what}: {} entries, the model has {N}", from.len()))
        }
        Ok(Self {
            config: take("config", &state.config)?,
            group1: take("group1", &state.group1)?,
            enabled: take("enabled", &state.enabled)?,
            pending: take("pending", &state.pending)?,
            active: take("active", &state.active)?,
            priority: take("priority", &state.priority)?,
        })
    }
}

/// One CPU interface as a snapshot keeps it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CpuInterfaceState {
    pub private: InterruptsState,
    pub sre: u64,
    pub pmr: u8,
    pub ctlr: u32,
    pub bpr0: u32,
    pub bpr1: u32,
    pub igrpen0: bool,
    pub igrpen1: bool,
    pub ap0r: [u32; 4],
    pub ap1r: [u32; 4],
    pub running_priority: u8,
    pub waker: u32,
}

/// The whole controller as a snapshot keeps it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GicState {
    pub ctlr: u32,
    pub spis: InterruptsState,
    pub route: Vec<usize>,
    pub cpus: Vec<CpuInterfaceState>,
}

/// The userspace GICv3: a distributor, one redistributor per vCPU, and the
/// routing state that ties SPIs to vCPUs.
#[derive(Debug, Clone)]
pub struct Gic {
    /// `GICD_CTLR`.
    ctlr: u32,
    /// The distributor's SPIs.
    spis: Interrupts<{ NUM_INTIDS - NUM_PRIVATE_INTIDS }>,
    /// `GICD_IROUTER<n>`: the affinity each SPI is routed to, or an affinity
    /// that names no vCPU of this machine (which leaves the interrupt
    /// unrouted, see [`Gic::set_route`]).
    route: [usize; NUM_INTIDS],
    /// The per-vCPU redistributors and CPU interfaces.
    cpus: Vec<CpuInterface>,
}

/// Which register group of the distributor or of a redistributor's SGI frame a
/// write addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BitRegister {
    Group,
    SetEnabled,
    ClearEnabled,
    SetPending,
    ClearPending,
    SetActive,
    ClearActive,
}

/// Classifies `offset` as one of the bit-per-interrupt register groups, if it
/// is one.
fn bit_register(offset: u64) -> Option<(BitRegister, u64)> {
    match offset {
        o if (GICD_IGROUPR..GICD_IGROUPR + 16).contains(&o) => {
            Some((BitRegister::Group, GICD_IGROUPR))
        }
        o if (GICD_ISENABLER..GICD_ISENABLER + 16).contains(&o) => {
            Some((BitRegister::SetEnabled, GICD_ISENABLER))
        }
        o if (GICD_ICENABLER..GICD_ICENABLER + 16).contains(&o) => {
            Some((BitRegister::ClearEnabled, GICD_ICENABLER))
        }
        o if (GICD_ISPENDR..GICD_ISPENDR + 16).contains(&o) => {
            Some((BitRegister::SetPending, GICD_ISPENDR))
        }
        o if (GICD_ICPENDR..GICD_ICPENDR + 16).contains(&o) => {
            Some((BitRegister::ClearPending, GICD_ICPENDR))
        }
        o if (GICD_ISACTIVER..GICD_ISACTIVER + 16).contains(&o) => {
            Some((BitRegister::SetActive, GICD_ISACTIVER))
        }
        o if (GICD_ICACTIVER..GICD_ICACTIVER + 16).contains(&o) => {
            Some((BitRegister::ClearActive, GICD_ICACTIVER))
        }
        _ => None,
    }
}

/// Reads the `size`-byte lane of `word` that starts at `offset`.
fn lane_extract(word: u32, offset: u64, size: u8) -> u64 {
    let shift = (offset % 4) * 8;
    let value = u64::from(word) >> shift;
    value & size_mask(size)
}

/// Writes the `size`-byte lane `value` into `word` at `offset`.
fn word_insert(word: u32, offset: u64, size: u8, value: u64) -> u32 {
    let shift = (offset % 4) * 8;
    let mask = (size_mask(size) as u32) << shift;
    (word & !mask) | (u32::try_from(value & size_mask(size)).unwrap_or(0) << shift) & mask
}

/// A mask of the low `size` bytes.
const fn size_mask(size: u8) -> u64 {
    if size >= 8 {
        u64::MAX
    } else {
        (1u64 << (size * 8)) - 1
    }
}

impl Gic {
    /// Creates a controller for `vcpu_count` vCPUs (at least one).
    pub fn new(vcpu_count: usize) -> Self {
        let vcpu_count = vcpu_count.max(1);
        // With a single security state (which is what this model presents:
        // `GICD_CTLR.DS` may be set and no Group 0 exists) every interrupt
        // resets to Group 1 and disabled, and every routing register resets to
        // the first CPU. The guest programs all of this itself; the reset
        // values are the ones a single-security-state implementation uses.
        let mut spis = Interrupts::new();
        spis.group1 = [true; { NUM_INTIDS - NUM_PRIVATE_INTIDS }];
        let cpus = (0..vcpu_count)
            .map(|_| {
                let mut cpu = CpuInterface::new();
                cpu.private.group1 = [true; NUM_PRIVATE_INTIDS];
                cpu
            })
            .collect();
        Self {
            ctlr: 0,
            spis,
            route: [0; NUM_INTIDS],
            cpus,
        }
    }

    /// The controller's state, for a snapshot.
    pub fn state(&self) -> GicState {
        GicState {
            ctlr: self.ctlr,
            spis: self.spis.state(),
            route: self.route.to_vec(),
            cpus: self
                .cpus
                .iter()
                .map(|cpu| CpuInterfaceState {
                    private: cpu.private.state(),
                    sre: cpu.sre,
                    pmr: cpu.pmr,
                    ctlr: cpu.ctlr,
                    bpr0: cpu.bpr0,
                    bpr1: cpu.bpr1,
                    igrpen0: cpu.igrpen0,
                    igrpen1: cpu.igrpen1,
                    ap0r: cpu.ap0r,
                    ap1r: cpu.ap1r,
                    running_priority: cpu.running_priority,
                    waker: cpu.waker,
                })
                .collect(),
        }
    }

    /// A controller in the state a snapshot kept, for `vcpu_count` vCPUs.
    pub fn from_state(state: &GicState, vcpu_count: usize) -> Result<Self, String> {
        if state.cpus.len() != vcpu_count {
            return Err(format!(
                "the snapshot's controller serves {} vCPUs, not {vcpu_count}",
                state.cpus.len()
            ));
        }
        let route = <[usize; NUM_INTIDS]>::try_from(state.route.as_slice()).map_err(|_| {
            format!(
                "route: {} entries, the model has {NUM_INTIDS}",
                state.route.len()
            )
        })?;
        let cpus = state
            .cpus
            .iter()
            .map(|cpu| {
                let mut restored = CpuInterface {
                    private: Interrupts::from_state(&cpu.private)?,
                    sre: cpu.sre,
                    pmr: cpu.pmr,
                    ctlr: cpu.ctlr,
                    bpr0: cpu.bpr0,
                    bpr1: cpu.bpr1,
                    igrpen0: cpu.igrpen0,
                    igrpen1: cpu.igrpen1,
                    ap0r: cpu.ap0r,
                    ap1r: cpu.ap1r,
                    running_priority: cpu.running_priority,
                    waker: cpu.waker,
                };
                // Snapshots taken before this change stored the idle value as
                // DEFAULT_PRIORITY; the running priority comes from `ap1r`.
                restored.refresh_running_priority();
                Ok(restored)
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            ctlr: state.ctlr,
            spis: Interrupts::from_state(&state.spis)?,
            route,
            cpus,
        })
    }

    /// The number of vCPUs this controller was built for.
    pub fn vcpu_count(&self) -> usize {
        self.cpus.len()
    }

    /// The MMIO windows this controller answers: the distributor, then one
    /// redistributor window per vCPU. Used by the vCPU loop to route a data
    /// abort, and by the device tree to describe the controller.
    pub fn mmio_windows(&self) -> Vec<(u64, u64)> {
        let mut windows = vec![(GICV3_DIST_BASE, GICV3_DIST_SIZE)];
        for cpu in 0..self.cpus.len() {
            windows.push((
                GICV3_REDIST_BASE + GICV3_REDIST_STRIDE * cpu as u64,
                GICV3_REDIST_STRIDE,
            ));
        }
        windows
    }

    /// Which register of this controller `addr` addresses, for a data abort
    /// the vCPU loop handed here. `None` means no part of the controller
    /// answers at that address.
    pub fn decode_address(&self, addr: u64) -> Option<GicMmio> {
        if in_window(addr, GICV3_DIST_BASE, GICV3_DIST_SIZE) {
            return Some(GicMmio::Distributor {
                offset: addr - GICV3_DIST_BASE,
            });
        }
        let (base, len) = redist_window(self.cpus.len());
        if !in_window(addr, base, len) {
            return None;
        }
        let from_base = addr - base;
        Some(GicMmio::Redistributor {
            cpu: (from_base / GICV3_REDIST_STRIDE) as usize,
            offset: from_base % GICV3_REDIST_STRIDE,
        })
    }

    // -- interrupt sources ------------------------------------------------

    /// Raises the SPI numbered `spi` (the device tree's numbering, i.e. the
    /// interrupt ID minus [`SPI_BASE`]). Used by the serial console, whose
    /// interrupt is edge-triggered: each call is one interrupt event.
    pub fn raise_spi(&mut self, spi: u32) {
        let Some(intid) = spi_to_intid(spi) else {
            return;
        };
        self.spis.pending[intid as usize - NUM_PRIVATE_INTIDS] = true;
    }

    /// Lowers an SPI that was raised earlier.
    pub fn lower_spi(&mut self, spi: u32) {
        let Some(intid) = spi_to_intid(spi) else {
            return;
        };
        self.spis.pending[intid as usize - NUM_PRIVATE_INTIDS] = false;
    }

    /// Raises the private interrupt `intid` (0..32: an SGI or a PPI) on `cpu`.
    pub fn raise_private(&mut self, cpu: usize, intid: u32) {
        if let Some(cpu) = self.cpus.get_mut(cpu)
            && let Some(pending) = cpu.private.pending.get_mut(intid as usize)
        {
            *pending = true;
        }
    }

    /// Lowers a private interrupt on `cpu`.
    pub fn lower_private(&mut self, cpu: usize, intid: u32) {
        if let Some(cpu) = self.cpus.get_mut(cpu)
            && let Some(pending) = cpu.private.pending.get_mut(intid as usize)
        {
            *pending = false;
        }
    }

    /// Raises the virtual timer's interrupt on `cpu`: what the vCPU loop does
    /// when the guest's `CNTV_CTL_EL0` reports the timer has fired.
    ///
    /// The interrupt ID is [`VIRTUAL_TIMER_INTID`], not the device tree's PPI
    /// number; see that constant.
    pub fn raise_virtual_timer(&mut self, cpu: usize) {
        self.raise_private(cpu, VIRTUAL_TIMER_INTID);
    }

    /// Lowers the virtual timer's interrupt on `cpu`, once the guest has
    /// cleared the timer's condition.
    pub fn lower_virtual_timer(&mut self, cpu: usize) {
        self.lower_private(cpu, VIRTUAL_TIMER_INTID);
    }

    // -- delivery ---------------------------------------------------------

    /// Whether this CPU's interrupt line into HVF is asserted: an enabled
    /// Group 1 interrupt addressed to it is pending at a priority that can
    /// pass the CPU's priority mask.
    pub fn irq_line(&self, cpu: usize) -> bool {
        self.best_pending(cpu).is_some()
    }

    /// Whether the private interrupt `intid` of `cpu` would reach the guest if
    /// it became pending now: it is Group 1 and enabled in its redistributor,
    /// and the CPU interface would accept its priority.
    ///
    /// This is [`Gic::irq_line`]'s condition for one interrupt without it
    /// having to be pending, which is what the virtual timer needs: the
    /// timer's PPI is raised only once Hypervisor.framework has reported the
    /// timer, so a due timer is deliverable exactly when raising it would pass
    /// these checks. A vCPU that waits for a *due* timer whose interrupt
    /// cannot reach the guest would otherwise have to be run with a zero
    /// deadline, and would spin without executing a guest instruction.
    pub fn delivers_private(&self, cpu: usize, intid: u32) -> bool {
        let index = intid as usize;
        if index >= NUM_PRIVATE_INTIDS {
            return false;
        }
        let Some(state) = self.cpus.get(cpu) else {
            return false;
        };
        state.private.group1[index]
            && state.private.enabled[index]
            && state.accepts(state.private.priority[index])
    }

    /// Acknowledges the interrupt `cpu` should take (`ICC_IAR1_EL1`): the
    /// highest-priority pending interrupt addressed to it becomes active and
    /// stops being pending, and its ID is returned. [`SPURIOUS_INTID`] when
    /// there is nothing to take.
    pub fn ack(&mut self, cpu: usize) -> u32 {
        let Some((intid, priority)) = self.best_pending(cpu) else {
            return SPURIOUS_INTID;
        };
        if let Some(cpu_state) = self.cpus.get_mut(cpu) {
            if intid < SPI_BASE {
                cpu_state.private.pending[intid as usize] = false;
                cpu_state.private.active[intid as usize] = true;
            } else {
                self.spis.pending[intid as usize - NUM_PRIVATE_INTIDS] = false;
                self.spis.active[intid as usize - NUM_PRIVATE_INTIDS] = true;
            }
            // The interrupt's priority group stays active until the guest ends
            // it; the running priority follows the active groups.
            cpu_state.ap1r[0] |= 1u32 << (priority >> 3);
            cpu_state.refresh_running_priority();
        }
        intid
    }

    /// Ends the interrupt on `cpu` (`ICC_EOIR1_EL1`). It always drops the
    /// interrupt's priority, and it deactivates the interrupt as well when
    /// `ICC_CTLR_EL1.EOImode` is clear - which is what a guest running at EL1
    /// selects. With `EOImode` set, the guest deactivates separately through
    /// [`Gic::deactivate`].
    pub fn eoi(&mut self, cpu: usize, intid: u32) {
        let eoi_mode = self
            .cpus
            .get(cpu)
            .map_or(0, |state| state.ctlr & ICC_CTLR_EL1_EOIMODE);
        if eoi_mode == 0 {
            self.deactivate(cpu, intid);
        }
        if let Some(cpu_state) = self.cpus.get_mut(cpu) {
            // Priority drop: the highest active priority group (the lowest set
            // bit of `ap1r[0]`) stops being active, and the running priority
            // falls back to the next one still active, or to idle.
            cpu_state.ap1r[0] &= cpu_state.ap1r[0].wrapping_sub(1);
            cpu_state.refresh_running_priority();
        }
    }

    /// Deactivates an interrupt without ending it (`ICC_DIR_EL1`, used when
    /// `ICC_CTLR_EL1.EOImode` is set).
    pub fn deactivate(&mut self, cpu: usize, intid: u32) {
        if intid < SPI_BASE {
            if let Some(cpu_state) = self.cpus.get_mut(cpu)
                && let Some(active) = cpu_state.private.active.get_mut(intid as usize)
            {
                *active = false;
            }
        } else if let Some(active) = (intid as usize)
            .checked_sub(NUM_PRIVATE_INTIDS)
            .and_then(|i| self.spis.active.get_mut(i))
        {
            *active = false;
        }
    }

    /// The interrupt `cpu` should take next, if any: the pending, enabled
    /// Group 1 interrupt addressed to it with the lowest priority number.
    fn best_pending(&self, cpu: usize) -> Option<(u32, u8)> {
        let cpu_state = self.cpus.get(cpu)?;
        // Group 1 has to be enabled both in the distributor (for SPIs) and in
        // the CPU interface.
        let mut best: Option<(u32, u8)> = None;
        let mut consider =
            |intid: u32, priority: u8, group1: bool, enabled: bool, pending: bool| {
                if !(group1 && enabled && pending) {
                    return;
                }
                if !cpu_state.accepts(priority) {
                    return;
                }
                if best.is_none_or(|(_, best_priority)| priority < best_priority) {
                    best = Some((intid, priority));
                }
            };

        for (intid, _) in cpu_state.private.pending.iter().enumerate() {
            consider(
                intid as u32,
                cpu_state.private.priority[intid],
                cpu_state.private.group1[intid],
                cpu_state.private.enabled[intid],
                cpu_state.private.pending[intid],
            );
        }
        if self.ctlr & GICD_CTLR_ENABLE_GRP1 != 0 {
            for (spi, _) in self.spis.pending.iter().enumerate() {
                if self.route[SPI_BASE as usize + spi] != cpu {
                    continue;
                }
                consider(
                    SPI_BASE + spi as u32,
                    self.spis.priority[spi],
                    self.spis.group1[spi],
                    self.spis.enabled[spi],
                    self.spis.pending[spi],
                );
            }
        }
        best
    }

    /// Routes an SPI, as `GICD_IROUTER<n>` does: `value` holds the affinity
    /// (Aff0 in bits 7:0) and, in bit 31, the "any CPU" flag.
    fn set_route(&mut self, spi: u32, value: u64) {
        let Some(intid) = spi_to_intid(spi) else {
            return;
        };
        let affinity = value as u32;
        let target = if affinity & GICD_IROUTER_IRM != 0 {
            0
        } else {
            (affinity & 0xff) as usize
        };
        // Stored as written, including an affinity that names no vCPU of this
        // machine (vCPUs are numbered by their affinity level 0 and the device
        // tree's CPU nodes use that numbering). Such a route stays unrouted:
        // no vCPU has that index, so `best_pending` never delivers the
        // interrupt to anyone. Retargeting it to CPU 0 - which an earlier
        // revision of this model did - would deliver an interrupt the guest
        // routed elsewhere, on a CPU that enabled it.
        self.route[intid as usize] = target;
    }

    // -- distributor MMIO -------------------------------------------------

    /// Reads `size` bytes at `offset` from the distributor.
    pub fn dist_read(&self, offset: u64, size: u8) -> u64 {
        let offset = offset % GICV3_DIST_SIZE;
        if size == 8 {
            // `GICD_IROUTER<n>` is the only 64-bit distributor register.
            if let Some(spi) = router_spi(offset) {
                return self.router_value(spi);
            }
            let low = self.dist_read(offset, 4);
            let high = self.dist_read(offset + 4, 4);
            return low | (high << 32);
        }
        let word = self.dist_word(offset);
        lane_extract(word, offset, size)
    }

    /// Writes `size` bytes at `offset` on the distributor.
    pub fn dist_write(&mut self, offset: u64, size: u8, value: u64) {
        let offset = offset % GICV3_DIST_SIZE;
        if size == 8 {
            if let Some(spi) = router_spi(offset) {
                self.set_route(spi, value);
                return;
            }
            self.dist_write(offset, 4, value & 0xffff_ffff);
            self.dist_write(offset + 4, 4, value >> 32);
            return;
        }

        if let Some((register, base)) = bit_register(offset) {
            self.spis
                .write_bits(base, offset, size, value, |spis, intid| match register {
                    BitRegister::Group => spis.group1[intid] = true,
                    BitRegister::SetEnabled => spis.enabled[intid] = true,
                    BitRegister::ClearEnabled => spis.enabled[intid] = false,
                    BitRegister::SetPending => spis.pending[intid] = true,
                    BitRegister::ClearPending => spis.pending[intid] = false,
                    BitRegister::SetActive => spis.active[intid] = true,
                    BitRegister::ClearActive => spis.active[intid] = false,
                });
            return;
        }
        match offset {
            GICD_CTLR => {
                // `RWP` is read-only zero, so a write to it is ignored.
                self.ctlr = word_insert(self.ctlr, offset, size, value) & !GICD_CTLR_RWP;
            }
            o if (GICD_IPRIORITYR..GICD_IPRIORITYR + 128).contains(&o) => {
                self.spis.write_priority(offset, size, value);
            }
            o if (GICD_ICFGR..GICD_ICFGR + 32).contains(&o) => {
                self.spis.write_config(offset, size, value);
            }
            // `ICENABLER`-style registers above, the read-only identification
            // registers, and everything this model does not implement: writes
            // are ignored (RES0/WI), as on hardware.
            _ => {}
        }
    }

    /// The 32-bit distributor register that contains `offset`.
    fn dist_word(&self, offset: u64) -> u32 {
        if let Some((register, base)) = bit_register(offset) {
            let index = offset - base;
            return match register {
                BitRegister::Group => self.spis.read_bits(index, 4, |spis, intid| {
                    spis.group1.get(intid).copied().unwrap_or(false)
                }) as u32,
                BitRegister::SetEnabled | BitRegister::ClearEnabled => {
                    self.spis.read_bits(index, 4, |spis, intid| {
                        spis.enabled.get(intid).copied().unwrap_or(false)
                    }) as u32
                }
                BitRegister::SetPending | BitRegister::ClearPending => {
                    self.spis.read_bits(index, 4, |spis, intid| {
                        spis.pending.get(intid).copied().unwrap_or(false)
                    }) as u32
                }
                BitRegister::SetActive | BitRegister::ClearActive => {
                    self.spis.read_bits(index, 4, |spis, intid| {
                        spis.active.get(intid).copied().unwrap_or(false)
                    }) as u32
                }
            };
        }
        match offset {
            GICD_CTLR => self.ctlr,
            GICD_TYPER => {
                // One more ITLinesNumber than the number of 32-interrupt
                // blocks, 16 implemented interrupt-ID bits (field value 15),
                // no security extension (`DS` is the guest's business), no
                // LPIs, no message-based interrupts, no extended SPIs.
                (NUM_INTIDS as u32 / 32 - 1) | (15 << 19)
            }
            GICD_IIDR => 0,
            GICD_TYPER2 => 0,
            GICD_STATUSR => 0,
            GICD_PIDR2 => GIC_PIDR2_GICV3,
            o if (GICD_IPRIORITYR..GICD_IPRIORITYR + 128).contains(&o) => {
                self.spis.read_priority(offset, 4) as u32
            }
            o if (GICD_ICFGR..GICD_ICFGR + 32).contains(&o) => {
                self.spis.read_config(offset, 4) as u32
            }
            _ => 0,
        }
    }

    /// The 64-bit `GICD_IROUTER<n>` value of an SPI: the affinity of the vCPU
    /// it is routed to. This model has one cluster, so the value is the vCPU
    /// index in Aff0 and the "any CPU" flag is never set.
    fn router_value(&self, spi: u32) -> u64 {
        let Some(intid) = spi_to_intid(spi) else {
            return 0;
        };
        self.route[intid as usize] as u64
    }

    // -- redistributor MMIO -----------------------------------------------

    /// Reads `size` bytes at `offset` from the redistributor of `cpu`, where
    /// `offset` is relative to that redistributor's `RD_base`. Offsets inside
    /// the SGI frame (`+0x1_0000`) address the private interrupts.
    pub fn redist_read(&self, cpu: usize, offset: u64, size: u8) -> u64 {
        let offset = offset % GICV3_REDIST_STRIDE;
        let Some(state) = self.cpus.get(cpu) else {
            return 0;
        };
        if size == 8 {
            if offset == GICR_TYPER {
                return redist_typer(cpu, self.cpus.len());
            }
            let low = self.redist_read(cpu, offset, 4);
            let high = self.redist_read(cpu, offset + 4, 4);
            return low | (high << 32);
        }
        if offset >= GICV3_SGI_BASE_OFFSET {
            let sgi = offset - GICV3_SGI_BASE_OFFSET;
            return redist_private_word(state, sgi).map_or(0, |word| lane_extract(word, sgi, size));
        }
        let word = match offset {
            GICR_CTLR => 0,
            GICR_IIDR => 0,
            GICR_TYPER => (redist_typer(cpu, self.cpus.len()) & 0xffff_ffff) as u32,
            GICR_STATUSR => 0,
            GICR_WAKER => state.waker,
            GICR_PIDR2 => GIC_PIDR2_GICV3,
            _ => 0,
        };
        lane_extract(word, offset, size)
    }

    /// Writes `size` bytes at `offset` on the redistributor of `cpu`.
    pub fn redist_write(&mut self, cpu: usize, offset: u64, size: u8, value: u64) {
        let offset = offset % GICV3_REDIST_STRIDE;
        let Some(state) = self.cpus.get_mut(cpu) else {
            return;
        };
        if size == 8 {
            self.redist_write(cpu, offset, 4, value & 0xffff_ffff);
            self.redist_write(cpu, offset + 4, 4, value >> 32);
            return;
        }
        if offset >= GICV3_SGI_BASE_OFFSET {
            redist_private_write(state, offset - GICV3_SGI_BASE_OFFSET, size, value);
            return;
        }
        match offset {
            GICR_CTLR => {}
            GICR_WAKER => {
                state.waker = word_insert(state.waker, offset, size, value);
                // The redistributor wakes up immediately: no power domain of
                // its own exists here, so once the guest clears
                // `ProcessorSleep`, `ChildrenAsleep` reads back clear. The
                // kernel polls it in a loop with a one second timeout.
                if state.waker & GICR_WAKER_PROCESSOR_SLEEP == 0 {
                    state.waker &= !GICR_WAKER_CHILDREN_ASLEEP;
                } else {
                    state.waker |= GICR_WAKER_CHILDREN_ASLEEP;
                }
            }
            _ => {}
        }
    }

    // -- CPU interface system registers -----------------------------------

    /// Reads the system register `reg` (a hypervisor encoding, see
    /// [`sysreg`]) of `cpu`, if this controller owns it.
    pub fn sysreg_read(&self, cpu: usize, reg: u16) -> Option<u64> {
        let state = self.cpus.get(cpu)?;
        let value = match reg {
            ICC_SRE_EL1 => state.sre,
            ICC_PMR_EL1 => u64::from(state.pmr),
            ICC_CTLR_EL1 => u64::from(state.ctlr | ICC_CTLR_EL1_READ_ONLY),
            ICC_BPR0_EL1 => u64::from(state.bpr0),
            ICC_BPR1_EL1 => u64::from(state.bpr1),
            ICC_IGRPEN0_EL1 => u64::from(state.igrpen0),
            ICC_IGRPEN1_EL1 => u64::from(state.igrpen1),
            ICC_AP0R0_EL1 => u64::from(state.ap0r[0]),
            ICC_AP0R1_EL1 => u64::from(state.ap0r[1]),
            ICC_AP0R2_EL1 => u64::from(state.ap0r[2]),
            ICC_AP0R3_EL1 => u64::from(state.ap0r[3]),
            ICC_AP1R0_EL1 => u64::from(state.ap1r[0]),
            ICC_AP1R1_EL1 => u64::from(state.ap1r[1]),
            ICC_AP1R2_EL1 => u64::from(state.ap1r[2]),
            ICC_AP1R3_EL1 => u64::from(state.ap1r[3]),
            ICC_RPR_EL1 => u64::from(state.running_priority),
            // `IAR` and `HPPIR` are answered by `ack`/`highest_pending`,
            // because acknowledging has a side effect on the interrupt state.
            _ => return None,
        };
        Some(value)
    }

    /// Writes the system register `reg` of `cpu`. Returns `false` when this
    /// controller does not own the register, so the caller can leave it to
    /// HVF or fault the guest (the behaviour QEMU adopts for an unknown
    /// system register: an undefined-instruction exception).
    pub fn sysreg_write(&mut self, cpu: usize, reg: u16, value: u64) -> bool {
        // Acknowledge reads are handled by `ack`, so they are only accepted
        // here to keep `write`s on read-only registers a no-op.
        match reg {
            ICC_IAR0_EL1 | ICC_IAR1_EL1 | ICC_HPPIR0_EL1 | ICC_HPPIR1_EL1 => return true,
            ICC_SGI0R_EL1 | ICC_SGI1R_EL1 | ICC_ASGI1R_EL1 => {
                self.send_sgi(value);
                return true;
            }
            ICC_EOIR0_EL1 | ICC_EOIR1_EL1 => {
                self.eoi(cpu, (value & 0xffff_ffff) as u32);
                return true;
            }
            ICC_DIR_EL1 => {
                self.deactivate(cpu, (value & 0xffff_ffff) as u32);
                return true;
            }
            _ => {}
        }
        let Some(state) = self.cpus.get_mut(cpu) else {
            return false;
        };
        match reg {
            ICC_SRE_EL1 => {
                state.sre = value & (ICC_SRE_EL1_SRE | ICC_SRE_EL1_DFB | ICC_SRE_EL1_DIB)
            }
            ICC_PMR_EL1 => state.pmr = (value & 0xff) as u8,
            ICC_CTLR_EL1 => {
                // Only the writable fields are stored; the capability fields
                // are supplied on read.
                state.ctlr = (value as u32) & ICC_CTLR_EL1_WRITABLE;
            }
            ICC_BPR0_EL1 => state.bpr0 = (value & 0x7) as u32,
            ICC_BPR1_EL1 => state.bpr1 = (value & 0x7) as u32,
            ICC_IGRPEN0_EL1 => state.igrpen0 = value & 1 == 1,
            ICC_IGRPEN1_EL1 => state.igrpen1 = value & 1 == 1,
            ICC_AP0R0_EL1 => state.ap0r[0] = value as u32,
            ICC_AP0R1_EL1 => state.ap0r[1] = value as u32,
            ICC_AP0R2_EL1 => state.ap0r[2] = value as u32,
            ICC_AP0R3_EL1 => state.ap0r[3] = value as u32,
            ICC_AP1R0_EL1 => {
                state.ap1r[0] = value as u32;
                state.refresh_running_priority();
            }
            ICC_AP1R1_EL1 => state.ap1r[1] = value as u32,
            ICC_AP1R2_EL1 => state.ap1r[2] = value as u32,
            ICC_AP1R3_EL1 => state.ap1r[3] = value as u32,
            // `ICC_RPR_EL1` is read-only; a write is ignored on hardware.
            ICC_RPR_EL1 => {}
            _ => return false,
        }
        true
    }

    /// Acknowledges an interrupt for `cpu` (`ICC_IAR1_EL1` read).
    pub fn iar(&mut self, cpu: usize) -> u64 {
        u64::from(self.ack(cpu))
    }

    /// `ICC_HPPIR1_EL1`: the highest-priority pending interrupt, without
    /// taking it.
    pub fn hppir(&self, cpu: usize) -> u64 {
        u64::from(
            self.best_pending(cpu)
                .map_or(SPURIOUS_INTID, |(intid, _)| intid),
        )
    }

    /// Delivers a software-generated interrupt, as `ICC_SGI[01]R_EL1` does:
    /// the target list selects vCPUs 0..16 of a cluster and the affinity
    /// fields select the cluster. This model has a single cluster at affinity
    /// 0, so an SGI aimed at another cluster matches no vCPU, exactly as on
    /// hardware with no such cluster.
    fn send_sgi(&mut self, value: u64) {
        if value & (ICC_SGI1R_AFFINITY_1 | ICC_SGI1R_AFFINITY_2 | ICC_SGI1R_AFFINITY_3) != 0 {
            return;
        }
        let intid = ((value >> ICC_SGI1R_SGI_ID_SHIFT) & 0xf) as u32;
        let targets = (value >> ICC_SGI1R_TARGET_LIST_SHIFT) & 0xffff;
        for target in 0..self.cpus.len().min(16) {
            if targets >> target & 1 == 1 {
                self.raise_private(target, intid);
            }
        }
    }
}

/// The interrupt ID an SPI number names, if it is in range.
const fn spi_to_intid(spi: u32) -> Option<u32> {
    let intid = SPI_BASE + spi;
    if intid < NUM_INTIDS as u32 {
        Some(intid)
    } else {
        None
    }
}

/// The SPI number of a `GICD_IROUTER<n>` register at `offset`, if that is what
/// `offset` addresses.
const fn router_spi(offset: u64) -> Option<u32> {
    if offset < GICD_IROUTER || !(offset - GICD_IROUTER).is_multiple_of(8) {
        return None;
    }
    let spi = ((offset - GICD_IROUTER) / 8) as u32;
    if spi < NUM_INTIDS as u32 - SPI_BASE {
        Some(spi)
    } else {
        None
    }
}

/// Reads one 32-bit register of a redistributor's SGI frame.
fn redist_private_word(state: &CpuInterface, offset: u64) -> Option<u32> {
    let private = &state.private;
    if let Some((register, base)) = bit_register(offset) {
        let index = offset - base;
        return Some(match register {
            BitRegister::Group => private.read_bits(index, 4, |p, intid| {
                p.group1.get(intid).copied().unwrap_or(false)
            }) as u32,
            BitRegister::SetEnabled | BitRegister::ClearEnabled => {
                private.read_bits(index, 4, |p, intid| {
                    p.enabled.get(intid).copied().unwrap_or(false)
                }) as u32
            }
            BitRegister::SetPending | BitRegister::ClearPending => {
                private.read_bits(index, 4, |p, intid| {
                    p.pending.get(intid).copied().unwrap_or(false)
                }) as u32
            }
            BitRegister::SetActive | BitRegister::ClearActive => {
                private.read_bits(index, 4, |p, intid| {
                    p.active.get(intid).copied().unwrap_or(false)
                }) as u32
            }
        });
    }
    match offset {
        o if (GICD_IPRIORITYR..GICD_IPRIORITYR + 32).contains(&o) => {
            Some(private.read_priority(offset, 4) as u32)
        }
        o if (GICD_ICFGR..GICD_ICFGR + 8).contains(&o) => {
            Some(private.read_config(offset, 4) as u32)
        }
        GICD_PIDR2 => Some(GIC_PIDR2_GICV3),
        _ => Some(0),
    }
}

/// Writes one register of a redistributor's SGI frame, where `offset` is
/// relative to the frame's base.
fn redist_private_write(state: &mut CpuInterface, offset: u64, size: u8, value: u64) {
    let private = &mut state.private;
    if let Some((register, base)) = bit_register(offset) {
        private.write_bits(base, offset, size, value, |private, intid| match register {
            BitRegister::Group => private.group1[intid] = true,
            BitRegister::SetEnabled => private.enabled[intid] = true,
            BitRegister::ClearEnabled => private.enabled[intid] = false,
            BitRegister::SetPending => private.pending[intid] = true,
            BitRegister::ClearPending => private.pending[intid] = false,
            BitRegister::SetActive => private.active[intid] = true,
            BitRegister::ClearActive => private.active[intid] = false,
        });
        return;
    }
    match offset {
        o if (GICD_IPRIORITYR..GICD_IPRIORITYR + 32).contains(&o) => {
            private.write_priority(offset, size, value);
        }
        o if (GICD_ICFGR..GICD_ICFGR + 8).contains(&o) => private.write_config(offset, size, value),
        _ => {}
    }
}

/// `GICR_TYPER` of `cpu`: the CPU's affinity in bits 63:32 (the kernel matches
/// a redistributor to a CPU by comparing that field with the affinity of the
/// CPU it is initializing), the number of PPIs in bits 31:27 (a zero field,
/// which means "16 PPIs", what this model provides), and `Last` on the
/// region's last redistributor - which is how the kernel finds the end of a
/// redistributor region.
fn redist_typer(cpu: usize, vcpu_count: usize) -> u64 {
    let mut typer = (cpu as u64) << 32;
    if cpu + 1 == vcpu_count {
        typer |= u64::from(GICR_TYPER_LAST);
    }
    typer
}

// ---------------------------------------------------------------------------
// CPU interface system registers
// ---------------------------------------------------------------------------

/// The hypervisor encoding of a system register, shared with the exit decoder
/// that derives it from a trapped `MRS`/`MSR`:
/// `(op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2`.
pub use super::exit::sysreg;

/// `ICC_IAR0_EL1`: acknowledge a Group 0 interrupt.
pub const ICC_IAR0_EL1: u16 = sysreg(3, 0, 12, 8, 0);
/// `ICC_EOIR0_EL1`: end a Group 0 interrupt.
pub const ICC_EOIR0_EL1: u16 = sysreg(3, 0, 12, 8, 1);
/// `ICC_HPPIR0_EL1`: highest-priority pending Group 0 interrupt.
pub const ICC_HPPIR0_EL1: u16 = sysreg(3, 0, 12, 8, 2);
/// `ICC_BPR0_EL1`: binary point for Group 0.
pub const ICC_BPR0_EL1: u16 = sysreg(3, 0, 12, 8, 3);
/// `ICC_AP0R0_EL1`: active priorities, Group 0.
pub const ICC_AP0R0_EL1: u16 = sysreg(3, 0, 12, 8, 4);
/// `ICC_AP0R1_EL1`.
pub const ICC_AP0R1_EL1: u16 = sysreg(3, 0, 12, 8, 5);
/// `ICC_AP0R2_EL1`.
pub const ICC_AP0R2_EL1: u16 = sysreg(3, 0, 12, 8, 6);
/// `ICC_AP0R3_EL1`.
pub const ICC_AP0R3_EL1: u16 = sysreg(3, 0, 12, 8, 7);
/// `ICC_AP1R0_EL1`: active priorities, Group 1.
pub const ICC_AP1R0_EL1: u16 = sysreg(3, 0, 12, 9, 0);
/// `ICC_AP1R1_EL1`.
pub const ICC_AP1R1_EL1: u16 = sysreg(3, 0, 12, 9, 1);
/// `ICC_AP1R2_EL1`.
pub const ICC_AP1R2_EL1: u16 = sysreg(3, 0, 12, 9, 2);
/// `ICC_AP1R3_EL1`.
pub const ICC_AP1R3_EL1: u16 = sysreg(3, 0, 12, 9, 3);
/// `ICC_DIR_EL1`: deactivate an interrupt.
pub const ICC_DIR_EL1: u16 = sysreg(3, 0, 12, 11, 1);
/// `ICC_RPR_EL1`: the priority of the running interrupt.
pub const ICC_RPR_EL1: u16 = sysreg(3, 0, 12, 11, 3);
/// `ICC_SGI1R_EL1`: send a Group 1 software-generated interrupt.
pub const ICC_SGI1R_EL1: u16 = sysreg(3, 0, 12, 11, 5);
/// `ICC_ASGI1R_EL1`: send a Group 1 SGI to a named affinity at EL1.
pub const ICC_ASGI1R_EL1: u16 = sysreg(3, 0, 12, 11, 6);
/// `ICC_SGI0R_EL1`: send a Group 0 SGI.
pub const ICC_SGI0R_EL1: u16 = sysreg(3, 0, 12, 11, 7);
/// `ICC_IAR1_EL1`: acknowledge a Group 1 interrupt.
pub const ICC_IAR1_EL1: u16 = sysreg(3, 0, 12, 12, 0);
/// `ICC_EOIR1_EL1`: end a Group 1 interrupt.
pub const ICC_EOIR1_EL1: u16 = sysreg(3, 0, 12, 12, 1);
/// `ICC_HPPIR1_EL1`: highest-priority pending Group 1 interrupt.
pub const ICC_HPPIR1_EL1: u16 = sysreg(3, 0, 12, 12, 2);
/// `ICC_BPR1_EL1`: binary point for Group 1.
pub const ICC_BPR1_EL1: u16 = sysreg(3, 0, 12, 12, 3);
/// `ICC_CTLR_EL1`: control.
pub const ICC_CTLR_EL1: u16 = sysreg(3, 0, 12, 12, 4);
/// `ICC_SRE_EL1`: system-register interface enable.
pub const ICC_SRE_EL1: u16 = sysreg(3, 0, 12, 12, 5);
/// `ICC_IGRPEN0_EL1`: Group 0 enable.
pub const ICC_IGRPEN0_EL1: u16 = sysreg(3, 0, 12, 12, 6);
/// `ICC_IGRPEN1_EL1`: Group 1 enable.
pub const ICC_IGRPEN1_EL1: u16 = sysreg(3, 0, 12, 12, 7);
/// `ICC_PMR_EL1`: the priority mask.
pub const ICC_PMR_EL1: u16 = sysreg(3, 0, 4, 6, 0);

/// `ICC_SRE_EL1.SRE`, `.DFB` and `.DIB`: the guest sets all three as it takes
/// the system-register interface into use. All three are stored and read back
/// set, because this controller has no memory-mapped CPU interface to fall
/// back to.
pub const ICC_SRE_EL1_SRE: u64 = 1 << 0;
/// `ICC_SRE_EL1.DFB`.
pub const ICC_SRE_EL1_DFB: u64 = 1 << 1;
/// `ICC_SRE_EL1.DIB`.
pub const ICC_SRE_EL1_DIB: u64 = 1 << 2;

/// `ICC_CTLR_EL1.EOImode`: the guest sets it as it takes the split
/// end-of-interrupt/deactivate protocol into use.
const ICC_CTLR_EL1_EOIMODE: u32 = 1 << 1;
/// The fields of `ICC_CTLR_EL1` a guest may write.
const ICC_CTLR_EL1_WRITABLE: u32 = ICC_CTLR_EL1_EOIMODE | (1 << 0) /* CBPR */ | (1 << 6) /* PMHE */;

/// The read-only capability fields of `ICC_CTLR_EL1`: five implemented
/// priority bits (field value 4, as the kernels' vGIC uses), no ID range
/// extension, no range selector support, no GICv4 SEIS.
const ICC_CTLR_EL1_READ_ONLY: u32 = 4 << 8;

/// `ICC_SGI1R_EL1.TargetList`: the CPUs of the cluster an SGI goes to.
const ICC_SGI1R_TARGET_LIST_SHIFT: u64 = 0;
/// `ICC_SGI1R_EL1.Aff1`, the first cluster affinity field (bits 23:16). Aff2
/// (bits 39:32) and Aff3 (bits 55:48) complete the cluster's affinity. The
/// interrupt number lives in bits 27:24, inside this range, so the fields have
/// to be masked individually rather than as one shift.
const ICC_SGI1R_AFFINITY_1: u64 = 0xff << 16;
/// `ICC_SGI1R_EL1.Aff2`.
const ICC_SGI1R_AFFINITY_2: u64 = 0xff << 32;
/// `ICC_SGI1R_EL1.Aff3`.
const ICC_SGI1R_AFFINITY_3: u64 = 0xff << 48;
/// `ICC_SGI1R_EL1.IntID`: the SGI number.
const ICC_SGI1R_SGI_ID_SHIFT: u64 = 24;

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use super::*;

    #[test]
    fn the_controller_state_round_trips() {
        let mut gic = Gic::new(2);
        gic.dist_write(GICD_CTLR, 4, 0x13);
        gic.raise_spi(40);
        gic.raise_virtual_timer(1);
        let state = gic.state();
        let back = Gic::from_state(&state, 2).expect("restores");
        assert_eq!(back.state(), state);
        assert_eq!(back.irq_line(1), gic.irq_line(1));
        let json = serde_json::to_string(&state).expect("serialises");
        let parsed: GicState = serde_json::from_str(&json).expect("parses");
        assert_eq!(parsed, state);
        assert!(
            Gic::from_state(&state, 3).is_err(),
            "another vCPU count is refused"
        );
    }

    /// Locks a mutex, recovering from a peer thread's panic the way the rest
    /// of this tree does.
    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A controller for `n` vCPUs with Group 1 enabled in the distributor.
    fn gic(n: usize) -> Gic {
        let mut gic = Gic::new(n);
        gic.dist_write(GICD_CTLR, 4, u64::from(GICD_CTLR_ENABLE_GRP1));
        gic
    }

    /// Unmasks interrupts and enables Group 1 on `cpu`.
    fn unmask(gic: &mut Gic, cpu: usize) {
        gic.sysreg_write(cpu, ICC_PMR_EL1, u64::from(0xf0u8));
        gic.sysreg_write(cpu, ICC_IGRPEN1_EL1, 1);
    }

    /// Enables the SPI numbered `spi` and routes it to `cpu`.
    fn enable_spi(gic: &mut Gic, spi: u32, cpu: usize) {
        // `ICENABLER` defaults to disabled; enable, route, then raise.
        gic.dist_write(GICD_ISENABLER + (spi as u64 / 32) * 4, 4, 1 << (spi % 32));
        gic.dist_write(GICD_IROUTER + u64::from(spi) * 8, 8, cpu as u64);
    }

    #[test]
    fn sysreg_encoding_matches_the_hypervisor_encoding() {
        // Values from `Hypervisor/hv.h` (`HV_SYS_REG_*`): MIDR_EL1 = 0xc000,
        // MPIDR_EL1 = 0xc005, SCTLR_EL1 = 0xc080, CNTV_CTL_EL0 = 0xdf19.
        assert_eq!(sysreg(3, 0, 0, 0, 0), 0xc000);
        assert_eq!(sysreg(3, 0, 0, 0, 5), 0xc005);
        assert_eq!(sysreg(3, 0, 1, 0, 0), 0xc080);
        assert_eq!(sysreg(3, 3, 14, 3, 1), 0xdf19);
        assert_eq!(ICC_IAR1_EL1, 0xc660);
        assert_eq!(ICC_EOIR1_EL1, 0xc661);
        assert_eq!(ICC_CTLR_EL1, 0xc664);
        assert_eq!(ICC_SRE_EL1, 0xc665);
        assert_eq!(ICC_IGRPEN1_EL1, 0xc667);
        assert_eq!(ICC_PMR_EL1, 0xc230);
        assert_eq!(ICC_SGI1R_EL1, 0xc65d);
    }

    #[test]
    fn typer_reports_this_models_interrupt_range() {
        let gic = Gic::new(2);
        let typer = gic.dist_read(GICD_TYPER, 4) as u32;
        assert_eq!(typer & 0x1f, (NUM_INTIDS as u32 / 32) - 1);
        // 16 interrupt-ID bits, no security extension, no LPIs or MBIs.
        assert_eq!((typer >> 19) & 0x1f, 15);
        assert_eq!(typer & (1 << 10), 0);
        assert_eq!(typer & (1 << 17), 0);
        assert_eq!(gic.dist_read(GICD_PIDR2, 4) as u32 & 0xf0, GIC_PIDR2_GICV3);
    }

    #[test]
    fn an_spi_only_reaches_its_routed_cpu() {
        let mut gic = gic(2);
        unmask(&mut gic, 0);
        unmask(&mut gic, 1);
        enable_spi(&mut gic, UART_SPI, 1);

        gic.raise_spi(UART_SPI);
        assert!(!gic.irq_line(0), "the SPI is routed to vCPU 1");
        assert!(gic.irq_line(1));

        assert_eq!(gic.ack(1), UART_INTID);
        assert!(!gic.irq_line(1), "acknowledging clears the pending bit");
        assert_eq!(gic.ack(1), SPURIOUS_INTID);

        gic.eoi(1, UART_INTID);
        gic.raise_spi(UART_SPI);
        assert!(gic.irq_line(1), "an edge raises the line again after EOI");
    }

    #[test]
    fn delivery_needs_the_distributor_the_group_and_the_priority_mask() {
        let mut gic = gic(1);
        enable_spi(&mut gic, UART_SPI, 0);
        gic.raise_spi(UART_SPI);

        // The guest has not enabled Group 1 in the CPU interface yet.
        assert!(!gic.irq_line(0));
        unmask(&mut gic, 0);
        assert!(gic.irq_line(0));

        // A priority mask at or below the interrupt's priority masks it, and
        // one above it lets it through: an interrupt is signalled when its
        // priority number is lower than the mask.
        gic.sysreg_write(0, ICC_PMR_EL1, u64::from(DEFAULT_PRIORITY));
        assert!(!gic.irq_line(0), "the mask is at the interrupt's priority");
        gic.sysreg_write(0, ICC_PMR_EL1, u64::from(DEFAULT_PRIORITY + 8));
        assert!(gic.irq_line(0), "a higher mask passes the interrupt");

        // The distributor can be switched off too.
        gic.dist_write(GICD_CTLR, 4, 0);
        assert!(!gic.irq_line(0));

        // ... and disabling the interrupt hides it as well.
        gic.dist_write(GICD_CTLR, 4, u64::from(GICD_CTLR_ENABLE_GRP1));
        gic.dist_write(GICD_ICENABLER, 4, 1 << UART_SPI);
        assert!(!gic.irq_line(0));
    }

    #[test]
    fn priorities_are_byte_addressable_and_read_back() {
        let mut gic = gic(1);
        // The kernel writes one byte per SPI priority.
        gic.dist_write(GICD_IPRIORITYR + u64::from(UART_SPI), 1, 0xd0);
        let word = gic.dist_read(GICD_IPRIORITYR + (u64::from(UART_SPI) & !3), 4) as u32;
        assert_eq!((word >> ((UART_SPI % 4) * 8)) & 0xff, 0xd0);

        // ... and four bytes at a time while resetting the range.
        gic.dist_write(GICD_IPRIORITYR, 4, 0xa0a0_a0a0);
        assert_eq!(gic.dist_read(GICD_IPRIORITYR, 4) as u32, 0xa0a0_a0a0);
    }

    #[test]
    fn the_highest_priority_pending_interrupt_wins() {
        let mut gic = gic(1);
        unmask(&mut gic, 0);
        enable_spi(&mut gic, UART_SPI, 0);
        // Two enabled SPIs, the lower priority number being more urgent.
        gic.dist_write(GICD_ISENABLER, 4, 1 << 3);
        gic.dist_write(GICD_IROUTER + 3 * 8, 8, 0);
        gic.dist_write(GICD_IPRIORITYR + u64::from(UART_SPI), 1, 0xc0);
        gic.dist_write(GICD_IPRIORITYR + 3, 1, 0x80);

        gic.raise_spi(3);
        gic.raise_spi(UART_SPI);
        assert_eq!(gic.hppir(0), u64::from(SPI_BASE + 3));
        assert_eq!(gic.ack(0), SPI_BASE + 3);
        // The UART's priority is lower than the running one, so it waits for
        // the end of SPI 3's handler.
        gic.eoi(0, SPI_BASE + 3);
        assert_eq!(gic.hppir(0), u64::from(UART_INTID));
        assert_eq!(gic.ack(0), UART_INTID);
        assert_eq!(gic.ack(0), SPURIOUS_INTID);
    }

    #[test]
    fn an_active_spi_is_not_signalled_again_before_eoi() {
        let mut gic = gic(1);
        unmask(&mut gic, 0);
        enable_spi(&mut gic, 2, 0);
        gic.raise_spi(2);
        assert_eq!(gic.ack(0), SPI_BASE + 2);

        // The edge is raised again while the handler runs. The interrupt is
        // still active, so it is not signalled until its end-of-interrupt.
        gic.raise_spi(2);
        assert!(!gic.irq_line(0));
        assert_eq!(gic.ack(0), SPURIOUS_INTID);

        gic.eoi(0, SPI_BASE + 2);
        assert!(gic.irq_line(0));
        assert_eq!(gic.ack(0), SPI_BASE + 2);
    }

    #[test]
    fn a_higher_priority_spi_preempts_a_running_one() {
        let mut gic = gic(1);
        unmask(&mut gic, 0);
        enable_spi(&mut gic, 2, 0);
        enable_spi(&mut gic, 8, 0);
        gic.dist_write(GICD_IPRIORITYR + 2, 1, 0xc0);
        gic.dist_write(GICD_IPRIORITYR + 8, 1, 0x80);

        gic.raise_spi(2);
        assert_eq!(gic.ack(0), SPI_BASE + 2);
        assert_eq!(gic.sysreg_read(0, ICC_RPR_EL1), Some(0xc0));

        // SPI 8 is more urgent than the running SPI 2, so it preempts it.
        gic.raise_spi(8);
        assert_eq!(gic.ack(0), SPI_BASE + 8);
        assert_eq!(gic.sysreg_read(0, ICC_RPR_EL1), Some(0x80));

        gic.eoi(0, SPI_BASE + 8);
        assert_eq!(gic.sysreg_read(0, ICC_RPR_EL1), Some(0xc0));

        gic.eoi(0, SPI_BASE + 2);
        assert_eq!(gic.sysreg_read(0, ICC_RPR_EL1), Some(0xff));
    }

    #[test]
    fn the_console_and_the_virtio_spis_are_delivered_independently() {
        let mut gic = gic(1);
        unmask(&mut gic, 0);
        // The console's SPI and the virtio devices' SPIs: vsock (1), the
        // workspace (2) and a data volume (8), all routed to CPU 0.
        let spis = [UART_SPI, 1, 2, 8];
        for spi in spis {
            enable_spi(&mut gic, spi, 0);
        }
        for spi in spis {
            gic.raise_spi(spi);
        }

        let mut acked = Vec::new();
        for _ in 0..4 {
            let intid = gic.ack(0);
            assert_ne!(intid, SPURIOUS_INTID, "every raised SPI is taken");
            acked.push(intid);
            gic.eoi(0, intid);
        }
        acked.sort_unstable();
        let mut expected: Vec<u32> = spis.iter().map(|spi| SPI_BASE + spi).collect();
        expected.sort_unstable();
        assert_eq!(acked, expected, "each SPI is delivered exactly once");
        assert_eq!(gic.ack(0), SPURIOUS_INTID);
    }

    #[test]
    fn eoi_mode_selects_whether_eoi_deactivates() {
        let mut gic = gic(1);
        unmask(&mut gic, 0);
        enable_spi(&mut gic, UART_SPI, 0);
        gic.raise_spi(UART_SPI);
        assert_eq!(gic.ack(0), UART_INTID);

        // EOImode = 1: EOI only drops the priority; the interrupt stays
        // active until ICC_DIR_EL1 writes it away.
        gic.sysreg_write(0, ICC_CTLR_EL1, 1 << 1);
        gic.sysreg_write(0, ICC_EOIR1_EL1, u64::from(UART_INTID));
        assert!(gic.spis.active[UART_SPI as usize]);
        gic.sysreg_write(0, ICC_DIR_EL1, u64::from(UART_INTID));
        assert!(!gic.spis.active[UART_SPI as usize]);

        // EOImode = 0 (what a guest at EL1 selects): EOI deactivates.
        gic.sysreg_write(0, ICC_CTLR_EL1, 0);
        gic.raise_spi(UART_SPI);
        assert_eq!(gic.ack(0), UART_INTID);
        gic.sysreg_write(0, ICC_EOIR1_EL1, u64::from(UART_INTID));
        assert!(!gic.spis.active[UART_SPI as usize]);
    }

    #[test]
    fn the_redistributor_wakes_up_and_reports_its_affinity() {
        let mut gic = Gic::new(2);

        for cpu in 0..2 {
            let waker = gic.redist_read(cpu, GICR_WAKER, 4) as u32;
            assert_eq!(
                waker & GICR_WAKER_PROCESSOR_SLEEP,
                GICR_WAKER_PROCESSOR_SLEEP
            );
            assert_eq!(
                waker & GICR_WAKER_CHILDREN_ASLEEP,
                GICR_WAKER_CHILDREN_ASLEEP
            );
        }

        // The kernel clears ProcessorSleep and then polls for ChildrenAsleep
        // to clear, in a loop with a one second timeout.
        for cpu in 0..2 {
            let waker = gic.redist_read(cpu, GICR_WAKER, 4) as u32;
            gic.redist_write(
                cpu,
                GICR_WAKER,
                4,
                u64::from(waker & !GICR_WAKER_PROCESSOR_SLEEP),
            );
            let waker = gic.redist_read(cpu, GICR_WAKER, 4) as u32;
            assert_eq!(waker & GICR_WAKER_PROCESSOR_SLEEP, 0);
            assert_eq!(waker & GICR_WAKER_CHILDREN_ASLEEP, 0);
        }

        // The kernel matches a redistributor to a CPU by comparing the
        // affinity in TYPER's upper half with the CPU's affinity.
        assert_eq!(gic.redist_read(0, GICR_TYPER, 8) >> 32, 0);
        assert_eq!(gic.redist_read(1, GICR_TYPER, 8) >> 32, 1);
        assert_eq!(
            gic.redist_read(0, GICR_TYPER, 8) as u32 & GICR_TYPER_LAST,
            0
        );
        assert_ne!(
            gic.redist_read(1, GICR_TYPER, 8) as u32 & GICR_TYPER_LAST,
            0
        );

        // 16 PPIs, i.e. a zero in the NR_PPIs field.
        assert_eq!((gic.redist_read(0, GICR_TYPER, 8) as u32 >> 27) & 0x1f, 0);
        assert_eq!(
            gic.redist_read(0, GICR_PIDR2, 4) as u32 & 0xf0,
            GIC_PIDR2_GICV3
        );
    }

    #[test]
    fn private_interrupts_are_per_cpu() {
        let mut gic = gic(2);
        for cpu in 0..2 {
            unmask(&mut gic, cpu);
            // The kernel enables the PPIs it uses through the redistributor.
            // Only the virtual timer's PPI is enabled here - at its interrupt
            // ID, which is what a guest enables (see
            // `the_virtual_timer_is_raised_at_the_intid_the_guest_enables`) -
            // so this test can tell that ID from the device tree's PPI number.
            gic.redist_write(
                cpu,
                GICV3_SGI_BASE_OFFSET + GICD_ISENABLER,
                4,
                1 << VIRTUAL_TIMER_INTID,
            );
        }

        gic.raise_virtual_timer(1);
        assert!(!gic.irq_line(0), "PPIs belong to one CPU only");
        assert!(gic.irq_line(1));
        assert_eq!(gic.ack(1), VIRTUAL_TIMER_INTID);
        assert!(!gic.irq_line(1));
        gic.eoi(1, VIRTUAL_TIMER_INTID);

        // The device tree's PPI number is not the interrupt ID: raising it
        // raises an SGI, and the guest has not enabled that.
        gic.raise_private(0, VIRTUAL_TIMER_PPI);
        assert!(
            !gic.irq_line(0),
            "INTID {VIRTUAL_TIMER_PPI} is an SGI, not the timer's PPI"
        );

        // The guest can also raise a PPI by writing its pending register.
        gic.redist_write(
            0,
            GICV3_SGI_BASE_OFFSET + GICD_ISPENDR,
            4,
            1 << VIRTUAL_TIMER_INTID,
        );
        assert!(gic.irq_line(0));
    }

    #[test]
    fn the_virtual_timer_is_raised_at_the_intid_the_guest_enables() {
        // The device tree declares the architected virtual timer as PPI 11
        // (`arch::aarch64::fdt`), and the kernel translates a device tree PPI
        // n into hardware interrupt n + 16 (`drivers/irqchip/irq-gic-v3.c`):
        // the guest enables bit 27 of `GICR_ISENABLER0` and reads 27 from
        // `ICC_IAR1_EL1`. Raising the device tree's number pends an SGI the
        // guest never enabled, and a Linux guest never receives its tick.
        assert_eq!(VIRTUAL_TIMER_PPI, 11, "the device tree's PPI number");
        assert_eq!(VIRTUAL_TIMER_INTID, 27);

        let mut gic = gic(1);
        unmask(&mut gic, 0);
        gic.redist_write(
            0,
            GICV3_SGI_BASE_OFFSET + GICD_ISENABLER,
            4,
            1 << VIRTUAL_TIMER_INTID,
        );

        gic.raise_virtual_timer(0);
        assert!(gic.irq_line(0), "the enabled timer PPI delivers");
        assert_eq!(gic.ack(0), VIRTUAL_TIMER_INTID, "the guest acks INTID 27");

        // The line goes away again once the guest has cleared the timer.
        gic.raise_virtual_timer(0);
        gic.lower_virtual_timer(0);
        assert!(!gic.irq_line(0));
    }

    #[test]
    fn a_private_interrupt_is_deliverable_only_when_it_can_reach_the_guest() {
        // What a due virtual timer asks: whether raising its PPI *now* would
        // reach the guest. Group 1 has to be enabled in the CPU interface
        // (`ICC_IGRPEN1_EL1`), the PPI has to be enabled in its redistributor,
        // and the priority mask has to pass it (`ICC_PMR_EL1`, whose reset
        // value is 0 - it masks everything).
        let mut gic = gic(1);
        assert!(
            !gic.delivers_private(0, VIRTUAL_TIMER_INTID),
            "Group 1 is off in the CPU interface"
        );

        gic.sysreg_write(0, ICC_IGRPEN1_EL1, 1);
        assert!(
            !gic.delivers_private(0, VIRTUAL_TIMER_INTID),
            "the redistributor has not enabled the PPI"
        );

        gic.redist_write(
            0,
            GICV3_SGI_BASE_OFFSET + GICD_ISENABLER,
            4,
            1 << VIRTUAL_TIMER_INTID,
        );
        assert!(
            !gic.delivers_private(0, VIRTUAL_TIMER_INTID),
            "the reset priority mask (0x00) masks everything"
        );

        unmask(&mut gic, 0);
        assert!(gic.delivers_private(0, VIRTUAL_TIMER_INTID));

        // A mask at the interrupt's own priority masks it again, and one below
        // it does not.
        gic.sysreg_write(0, ICC_PMR_EL1, u64::from(DEFAULT_PRIORITY));
        assert!(!gic.delivers_private(0, VIRTUAL_TIMER_INTID));
        gic.sysreg_write(0, ICC_PMR_EL1, u64::from(DEFAULT_PRIORITY + 1));
        assert!(gic.delivers_private(0, VIRTUAL_TIMER_INTID));

        // An interrupt ID outside the private range is never deliverable this
        // way (an SPI is routed through the distributor instead).
        assert!(!gic.delivers_private(0, UART_INTID));
        // ... and neither is a private interrupt of a vCPU that does not
        // exist.
        assert!(!gic.delivers_private(9, VIRTUAL_TIMER_INTID));
    }

    #[test]
    fn an_spi_routed_to_an_absent_cpu_is_delivered_to_nobody() {
        let mut gic = gic(2);
        for cpu in 0..2 {
            unmask(&mut gic, cpu);
        }

        // The guest routes the console's SPI to affinity 5, which names no
        // vCPU of this machine. The architecture leaves such an interrupt
        // unrouted: delivering it to CPU 0 would hand a CPU an interrupt the
        // guest did not route to it (and that it may well have enabled).
        enable_spi(&mut gic, UART_SPI, 5);
        gic.raise_spi(UART_SPI);
        assert!(!gic.irq_line(0), "CPU 0 was not the routing target");
        assert!(!gic.irq_line(1), "CPU 1 was not the routing target");
        assert_eq!(gic.ack(0), SPURIOUS_INTID);
        assert_eq!(gic.ack(1), SPURIOUS_INTID);

        // `GICD_IROUTER` reads back the affinity the guest wrote, and routing
        // the SPI to a vCPU that exists delivers it there again.
        assert_eq!(gic.dist_read(GICD_IROUTER + u64::from(UART_SPI) * 8, 8), 5);
        gic.dist_write(GICD_IROUTER + u64::from(UART_SPI) * 8, 8, 1);
        assert!(!gic.irq_line(0));
        assert!(gic.irq_line(1));
        assert_eq!(gic.ack(1), UART_INTID);
    }

    #[test]
    fn a_guest_driven_change_wakes_the_peer_it_asserts() {
        use std::sync::mpsc::channel;
        use std::thread;
        use std::time::Duration;

        use crate::hvf::wait::{WaitSlot, wake_asserted};

        // A guest exit that changes the controller - an SGI aimed at another
        // vCPU, or a write to a redistributor's `ISPENDR`/`ISENABLER` - has to
        // wake the vCPU it targeted. Without that, an SGI to an idle core
        // leaves it asleep until an unrelated event (the console, the timer)
        // happens to arrive, which hangs a multi-vCPU guest in normal
        // operation.
        let mut gic = gic(2);
        for cpu in 0..2 {
            unmask(&mut gic, cpu);
            gic.redist_write(cpu, GICV3_SGI_BASE_OFFSET + GICD_ISENABLER, 4, !0);
        }
        let gic = Arc::new(Mutex::new(gic));

        let slots: Vec<Arc<WaitSlot>> = (0..2).map(|_| Arc::new(WaitSlot::new(true))).collect();
        let (woken, woken_rx) = channel();
        let halted = {
            let slot = Arc::clone(&slots[1]);
            let gic = Arc::clone(&gic);
            let woken = woken.clone();
            thread::spawn(move || {
                // The vCPU parks exactly as `vcpu_loop` does: until its own
                // line is asserted. It cannot report before the SGI below,
                // because nothing else asserts vCPU 1's line.
                slot.halt(None, || lock(&gic).irq_line(1));
                let _ = woken.send(1);
            })
        };

        // vCPU 0 sends SGI 0 to vCPU 1 exactly as `handle_sys_reg` does, and
        // the asserted lines are woken exactly as `SharedVm::with_gic` does.
        let asserted = {
            let mut gic = lock(&gic);
            assert!(gic.sysreg_write(0, ICC_SGI1R_EL1, 1 << 1));
            let mut asserted = 0u64;
            for index in 0..slots.len() {
                if gic.irq_line(index) {
                    asserted |= 1 << index;
                }
            }
            asserted
        };
        assert_eq!(asserted, 0b10, "only the targeted vCPU is asserted");
        wake_asserted(slots.iter().map(Arc::as_ref), asserted, |_| ());

        assert_eq!(
            woken_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            1,
            "the targeted vCPU woke"
        );
        halted.join().unwrap();
    }

    #[test]
    fn software_generated_interrupts_reach_the_target_list() {
        let mut gic = gic(2);
        for cpu in 0..2 {
            unmask(&mut gic, cpu);
            gic.redist_write(cpu, GICV3_SGI_BASE_OFFSET + GICD_ISENABLER, 4, !0);
        }

        // SGI 1 to CPU 1 (bit 1 of the target list), one cluster.
        let value = 1 << ICC_SGI1R_SGI_ID_SHIFT | 1 << 1;
        assert!(gic.sysreg_write(0, ICC_SGI1R_EL1, value));
        assert!(gic.irq_line(1));
        assert!(!gic.irq_line(0));
        assert_eq!(gic.ack(1), 1);

        // An SGI aimed at another cluster matches no vCPU of this machine.
        let other_cluster = 1 << ICC_SGI1R_SGI_ID_SHIFT | 1 << 1 | (1 << 16);
        assert!(gic.sysreg_write(0, ICC_SGI1R_EL1, other_cluster));
        assert!(!gic.irq_line(1));
    }

    #[test]
    fn unknown_system_registers_are_left_to_the_caller() {
        let mut gic = Gic::new(1);
        // A register this controller does not own must not be silently
        // swallowed: the caller faults the guest.
        let unrelated = sysreg(3, 0, 0, 0, 5); // MPIDR_EL1
        assert!(!gic.sysreg_write(0, unrelated, 0));
        assert_eq!(gic.sysreg_read(0, unrelated), None);
        assert!(gic.sysreg_write(0, ICC_SRE_EL1, ICC_SRE_EL1_SRE | ICC_SRE_EL1_DFB));
        assert_eq!(gic.sysreg_read(0, ICC_SRE_EL1), Some(0x3));
    }

    #[test]
    fn unknown_mmio_is_total() {
        let mut gic = Gic::new(1);
        // A reserved region inside a device's window reads as zero and
        // swallows writes.
        assert_eq!(gic.dist_read(0x2000, 4), 0);
        gic.dist_write(0x2000, 4, !0);
        assert_eq!(gic.dist_read(0x2000, 4), 0);
        assert_eq!(gic.redist_read(9, 0x100, 4), 0, "no such vCPU");
        assert_eq!(gic.redist_read(0, 0x8000, 4), 0);
        // A 64-bit read of two 32-bit registers is answered half by half: the
        // low half is the register at `offset`, the high half the next one.
        gic.dist_write(GICD_CTLR, 4, u64::from(GICD_CTLR_ENABLE_GRP1));
        let both = gic.dist_read(GICD_CTLR, 8);
        assert_eq!(both as u32, GICD_CTLR_ENABLE_GRP1);
        assert_eq!((both >> 32) as u32, gic.dist_read(GICD_TYPER, 4) as u32);
    }

    #[test]
    fn mmio_windows_cover_the_distributor_and_every_redistributor() {
        let gic = Gic::new(3);
        let windows = gic.mmio_windows();
        assert_eq!(windows.len(), 4);
        assert!(in_window(
            GICV3_DIST_BASE + GICV3_DIST_SIZE - 1,
            windows[0].0,
            windows[0].1
        ));
        assert!(!in_window(
            GICV3_DIST_BASE + GICV3_DIST_SIZE,
            windows[0].0,
            windows[0].1
        ));
        let (base, len) = windows[1];
        assert_eq!(base, GICV3_REDIST_BASE);
        assert_eq!(len, GICV3_REDIST_STRIDE);
        let (last, _) = windows[3];
        assert_eq!(last, GICV3_REDIST_BASE + 2 * GICV3_REDIST_STRIDE);
        assert_eq!(
            redist_window(3),
            (GICV3_REDIST_BASE, 3 * GICV3_REDIST_STRIDE)
        );
    }

    #[test]
    fn mmio_addresses_decode_to_the_right_register() {
        let gic = Gic::new(2);
        assert_eq!(
            gic.decode_address(GICV3_DIST_BASE + 0x100),
            Some(GicMmio::Distributor { offset: 0x100 })
        );
        // Each redistributor's SGI frame is `0x1_0000` past its RD_base.
        assert_eq!(
            gic.decode_address(GICV3_REDIST_BASE + GICV3_REDIST_STRIDE + 0x1_0004),
            Some(GicMmio::Redistributor {
                cpu: 1,
                offset: 0x1_0004
            })
        );
        // Outside the windows nothing answers, which is what the vCPU loop
        // turns into a fault in the guest.
        assert_eq!(gic.decode_address(GICV3_DIST_BASE - 4), None);
        assert_eq!(gic.decode_address(GICV3_DIST_BASE + GICV3_DIST_SIZE), None);
        let (base, len) = redist_window(2);
        assert_eq!(gic.decode_address(base + len), None);
        assert_eq!(gic.decode_address(0), None);
    }
}
