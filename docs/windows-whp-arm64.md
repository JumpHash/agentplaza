# Windows on arm64: agents' VMs on the Windows Hypervisor Platform

Status: design. Windows x64 boots the x86_64 guest through `crates/vmm/src/whp` (see
`docs/windows-whp.md`); Windows arm64 builds the platform layer but boots nothing yet. This is the
plan for the arm64 guest path, so Windows arm64 runs agents like every other platform except
Intel Macs.

## Reference

Microsoft's openvmm (MIT) drives WHP on arm64 and is the reference for the ABI, since the
Windows SDK header (`WinHvPlatformDefs.h`) is not on the Linux build box:

- `vm/whp/src/abi.rs`, `vm/whp/src/abi/arm64.rs`: register names, partition properties, exit
  reasons, `WHV_ARM64_IC_PARAMETERS`, `WHV_INTERRUPT_CONTROL`.
- `vm/hv1/hvdef/src/lib.rs`: the intercept messages carried in the exit context.
- `vmm_core/virt_whp/src/vp.rs` (`mod aarch64`): the run loop.

## What WHP does on arm64 (and what we no longer need)

- **GICv3 in the hypervisor.** Set `WHvPartitionPropertyCodeArm64IcParameters` (0x1012) before
  `WHvSetupPartition`: `{EmulationMode: GicV3 (1), Reserved: 0, GicV3Parameters: {GicdBaseAddress,
  GitsTranslatorBaseAddress: 0, Reserved: 0, GicLpiIntIdBits: 1, GicPpiOverflowInterruptFromCntv:
  0x14, GicPpiPerformanceMonitorsInterrupt: 0x17, Reserved1: [0; 6]}}` (packed). Each vCPU's
  redistributor goes in register `WHvArm64RegisterGicrBaseGpa` (0x00063000) = GICR base +
  index × 0x20000. The HVF backend's userspace GIC (`hvf/gic.rs`) is not used here.
- **PSCI in the hypervisor.** CPU_ON, CPU_OFF and friends are handled by WHP; SYSTEM_OFF and
  SYSTEM_RESET reach the VMM as `WHvRunVpExitReasonArm64Reset` (0x8001000c) with
  `HvArm64ResetInterceptMessage {header, reset_type (0 off, 1 reboot, 2 system reset, 3
  hibernate), reset_code}`. The device tree's PSCI node uses method "hvc".
- **Generic timer** (virtual timer PPI 27) is the hypervisor's.

## Exits the VMM handles

`WHV_RUN_VP_EXIT_CONTEXT` on arm64 is `{ExitReason: u32, Reserved: u32, Reserved1: u64, u:
[u8; 256] (align 16)}`; `u` starts with the hypervisor message payload, whose first 0x18 bytes are
`HvArm64InterceptMessageHeader {vp_index: u32, instruction_length: u8, intercept_access_type:
u8, execution_state: u16, pc: u64, cpsr: u64}`.

| exit | value | message | action |
|---|---|---|---|
| UnmappedGpa / GpaIntercept | 0x80000000 / 0x80000001 | `HvArm64MemoryInterceptMessage` (0x40 bytes): header, cache_type u32, instruction_byte_count u8, access_info u8, reserved u16, instruction_bytes [u8;4], reserved u32, gva u64, gpa u64, syndrome u64 | decode the ESR (`syndrome`) as a data abort (EC 0x24): ISV, SAS, SSE, SRT, SF, WnR — the HVF backend's `hvf/exit.rs` decoder; MMIO read → set Xsrt (sign-extend if SSE, mask to 32 if !SF), write → read Xsrt; then PC += 4 (IL) |
| Hypercall | 0x80000050 | `HvArm64HypercallInterceptMessage {header, immediate u16, reserved u16, flags u32, x: [u64; 18]}` | not expected (PSCI is the hypervisor's); fail loudly |
| Arm64Reset | 0x8001000c | `HvArm64ResetInterceptMessage` | stop the VM (off) or restart it (reboot/reset) |
| Canceled | 0xFFFFFFFF | — | `WHvCancelRunVirtualProcessor` from another thread (stop, or an interrupt to deliver) |

Registers: X0 = 0x00020000 + n (n 0..28), FP 0x0002001D, LR 0x0002001E, SP 0x0002001F, SpEl0
0x00020020, SpEl1 0x00020021, PC 0x00020022, Pstate 0x00020023; system registers in the 0x0004xxxx
range (SctlrEl1 0x00040002, MpidrEl1 0x00040001, …); full list in openvmm's `abi/arm64.rs`.

## Interrupts from devices

`WHvRequestInterrupt(partition, &WHV_INTERRUPT_CONTROL, size)` with the arm64 layout
`{TargetPartition: u64 (0), InterruptControl: u64, DestinationAddress: u64, RequestedVector: u32
(the SPI's INTID, 32 + n), TargetVtl: u8, ReservedZ0: u8, ReservedZ1: u16}`; `InterruptControl`
bit 34 (`INTERRUPT_CONTROL_ASSERTED`) asserts the line, clear deasserts it (level-triggered
virtio-mmio interrupts: assert on a used-ring update, deassert when the guest acks).

## Boot

Same guest as Linux arm64 and Apple Silicon (`target/guest/aarch64`): the `Image` kernel loaded
at the arch layout's kernel offset, the device tree from `crate::arch::aarch64::fdt` with a
`GicFdt` implementation for the WHP GIC (distributor + redistributor regions, the PPIs above) and
PSCI "hvc", X0 = the FDT's address, PC = the kernel entry, PSTATE = EL1h with DAIF masked
(0x3c5), MPIDR per vCPU. Secondary vCPUs are created stopped; the guest's PSCI CPU_ON starts them.

## Pieces

1. `whp/sys.rs` arm64: the register-name constants, the exit-context layout, `WHV_ARM64_IC_PARAMETERS`,
   `WHV_INTERRUPT_CONTROL` (arm64) — `#[cfg(target_arch = "aarch64")]`.
2. `whp/arm64/` pure half (testable anywhere): message parsing from byte slices, the ESR → MMIO
   access decision (reuse `hvf::exit`), the reset mapping.
3. `whp/arm64/boot.rs`, `vcpu.rs` (Windows aarch64 only): partition with the GIC property, memory
   map, FDT, vCPU threads, the run loop, device interrupts.
4. `spawner.rs`: route `all(windows, target_arch = "aarch64")` to it; `packaging/scripts/stage.sh`
   ships noded/vsockd/vmctl and the aarch64 guest for `aarch64-pc-windows-msvc`; the MSI includes them.
   Today `stage.sh` gives `aarch64-pc-windows-msvc` no VM node (its `*windows*` case sets `node=no`),
   so this step changes that case.
5. Verification: a boot on a Windows-on-Arm machine with the Hypervisor Platform feature
   (e.g. Snapdragon X). Today `.github/workflows/windows.yml` cross-compiles the arm64 build only
   when the `APZ_WINDOWS_ARM64` variable is `on`, and its tests are built, not run.
