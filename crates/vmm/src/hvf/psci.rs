// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. Firecracker's aarch64 port does not implement PSCI
// itself: the calls go to the kernel's in-kernel PSCI (KVM). The function
// encodings and return values below come from the PSCI specification and from
// `include/uapi/linux/psci.h`; the behaviour of each call matches what a VMM
// has to do (QEMU's `hvf_handle_psci_call()` in `target/arm/hvf/hvf.c`, which
// is GPL-2.0 and was read for its protocol decisions, not copied).

//! PSCI: the power-management calls the guest makes into the hypervisor.
//!
//! The device tree this port generates declares `arm,psci-0.2` with the `hvc`
//! conduit, so a guest call arrives as an [`Exception::Hvc`]
//! (see [`crate::hvf::exit`]) and is answered here: either with a value in
//! `X0`, or with an action the vCPU loop carries out (start another vCPU,
//! stop this VM).
//!
//! This module is free of FFI and of any `crate::arch` dependency so it builds
//! and is unit-tested on the host.

/// PSCI 1.1, the version this port reports.
pub const PSCI_VERSION_1_1: u64 = 0x0001_0001;

/// `PSCI_VERSION`.
pub const FN_PSCI_VERSION: u64 = 0x8400_0000;
/// `CPU_SUSPEND`, 32-bit and 64-bit forms.
pub const FN_CPU_SUSPEND: u64 = 0x8400_0001;
/// `CPU_SUSPEND` with 64-bit arguments, as a 64-bit guest sends it.
pub const FN64_CPU_SUSPEND: u64 = 0xc400_0001;
/// `CPU_OFF`.
pub const FN_CPU_OFF: u64 = 0x8400_0002;
/// `CPU_ON`.
pub const FN_CPU_ON: u64 = 0x8400_0003;
/// `CPU_ON` with 64-bit arguments, as a 64-bit guest sends it.
pub const FN64_CPU_ON: u64 = 0xc400_0003;
/// `AFFINITY_INFO`.
pub const FN_AFFINITY_INFO: u64 = 0x8400_0004;
/// `AFFINITY_INFO` with 64-bit arguments, as a 64-bit guest sends it.
pub const FN64_AFFINITY_INFO: u64 = 0xc400_0004;
/// `MIGRATE`.
pub const FN_MIGRATE: u64 = 0x8400_0005;
/// `MIGRATE_INFO_TYPE`.
pub const FN_MIGRATE_INFO_TYPE: u64 = 0x8400_0006;
/// `MIGRATE_INFO_UP_CPU`.
pub const FN_MIGRATE_INFO_UP_CPU: u64 = 0x8400_0007;
/// `SYSTEM_OFF`.
pub const FN_SYSTEM_OFF: u64 = 0x8400_0008;
/// `SYSTEM_RESET`.
pub const FN_SYSTEM_RESET: u64 = 0x8400_0009;
/// `PSCI_FEATURES`.
pub const FN_PSCI_FEATURES: u64 = 0x8400_000a;

/// `PSCI_RET_SUCCESS`.
pub const RET_SUCCESS: u64 = 0;
/// `PSCI_RET_NOT_SUPPORTED`, sign-extended as the SMCCC return value the guest
/// reads in `X0`.
pub const RET_NOT_SUPPORTED: u64 = -1i64 as u64;
/// `PSCI_RET_INVALID_PARAMS`.
pub const RET_INVALID_PARAMS: u64 = -2i64 as u64;
/// `PSCI_RET_ALREADY_ON`.
pub const RET_ALREADY_ON: u64 = -4i64 as u64;

/// `PSCI_0_2_AFFINITY_LEVEL_ON`.
pub const AFFINITY_LEVEL_ON: u64 = 0;
/// `PSCI_0_2_AFFINITY_LEVEL_OFF`.
pub const AFFINITY_LEVEL_OFF: u64 = 1;
/// `PSCI_0_2_TOS_MP`: "a trusted OS is deployed on multiple CPUs", the answer
/// a VMM without a trusted OS gives to `MIGRATE_INFO_TYPE`.
pub const MIGRATE_INFO_TOS_MP: u64 = 2;

/// The affinity-0 field of an `MPIDR`, which is how this port numbers vCPUs
/// (and what the device tree's CPU nodes give as their `reg`).
pub const fn affinity0(mpidr: u64) -> u64 {
    mpidr & 0xff
}

/// Whether an `MPIDR` names a vCPU of this machine at all: this port puts
/// every vCPU in one cluster, so affinity levels 1 to 3 must be zero.
pub const fn is_local_mpidr(mpidr: u64) -> bool {
    mpidr & !0xff == 0
}

/// The part of the machine state a PSCI call can observe.
#[derive(Debug, Clone, Copy)]
pub struct Machine<'a> {
    /// `running[cpu]` is true while that vCPU is on (it starts on for vCPU 0
    /// and off for the others, which wait for a `CPU_ON`).
    pub running: &'a [bool],
}

impl Machine<'_> {
    /// The number of vCPUs this machine has.
    pub fn vcpu_count(&self) -> usize {
        self.running.len()
    }

    /// Whether `mpidr` names a vCPU of this machine.
    fn cpu_index(&self, mpidr: u64) -> Option<usize> {
        if !is_local_mpidr(mpidr) {
            return None;
        }
        let index = affinity0(mpidr) as usize;
        (index < self.running.len()).then_some(index)
    }
}

/// What the vCPU loop has to do about a PSCI call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Put this value in `X0` and let the guest continue.
    Return(u64),
    /// Start the vCPU named by `mpidr` at `entry` with `X0 = context_id`.
    CpuOn {
        /// The `MPIDR` of the vCPU to start, whose affinity 0 is its index.
        mpidr: u64,
        /// The address the vCPU starts executing at.
        entry: u64,
        /// The value the vCPU finds in `X0`.
        context_id: u64,
    },
    /// The calling vCPU powers itself off and never returns from the call.
    CpuOff,
    /// Shut the VM down. `reset` distinguishes `SYSTEM_RESET` from
    /// `SYSTEM_OFF`; neither returns to the caller.
    SystemOff {
        /// `true` for `SYSTEM_RESET`, `false` for `SYSTEM_OFF`.
        reset: bool,
    },
    /// The calling vCPU suspends (a halt like `WFI`, resumed by an interrupt).
    /// The call reports success in `X0` before the vCPU idles.
    Suspend,
}

/// Answers a PSCI call: the function identifier and the first three arguments
/// from `X0..X3`.
pub fn handle(function: u64, a1: u64, a2: u64, a3: u64, machine: &Machine) -> Action {
    match function {
        FN_PSCI_VERSION => Action::Return(PSCI_VERSION_1_1),
        // A VMM with no trusted OS: the call is answered as unsupported
        // (QEMU answers the same way), and `MIGRATE_INFO_TYPE` reports a
        // trusted OS deployed on multiple CPUs, which is what makes a
        // migrating guest refuse to migrate - it never will here.
        FN_MIGRATE => Action::Return(RET_NOT_SUPPORTED),
        FN_MIGRATE_INFO_TYPE => Action::Return(MIGRATE_INFO_TOS_MP),
        FN_MIGRATE_INFO_UP_CPU => Action::Return(RET_NOT_SUPPORTED),

        FN_CPU_SUSPEND | FN64_CPU_SUSPEND => {
            // Only a power state whose identifier fits in the 16-bit field is
            // one this machine can enter: there are no affinities above level 0
            // and no retention states. The reference Hypervisor.framework
            // backend refuses the same states.
            if a1 & 0xfffe_0000 != 0 {
                Action::Return(RET_INVALID_PARAMS)
            } else {
                Action::Suspend
            }
        }

        FN_CPU_OFF => Action::CpuOff,

        FN_CPU_ON | FN64_CPU_ON => {
            let mpidr = a1;
            let Some(index) = machine.cpu_index(mpidr) else {
                return Action::Return(RET_INVALID_PARAMS);
            };
            if machine.running[index] {
                return Action::Return(RET_ALREADY_ON);
            }
            Action::CpuOn {
                mpidr,
                entry: a2,
                context_id: a3,
            }
        }

        FN_AFFINITY_INFO | FN64_AFFINITY_INFO => {
            let mpidr = a1;
            let level = a2;
            if level != 0 {
                // Everything above affinity level 0 is always on.
                return Action::Return(AFFINITY_LEVEL_ON);
            }
            match machine.cpu_index(mpidr) {
                Some(index) => Action::Return(if machine.running[index] {
                    AFFINITY_LEVEL_ON
                } else {
                    AFFINITY_LEVEL_OFF
                }),
                None => Action::Return(RET_INVALID_PARAMS),
            }
        }

        FN_SYSTEM_OFF => Action::SystemOff { reset: false },
        FN_SYSTEM_RESET => Action::SystemOff { reset: true },

        FN_PSCI_FEATURES => Action::Return(if supported(a1) {
            RET_SUCCESS
        } else {
            RET_NOT_SUPPORTED
        }),

        // SMCCC 1.3 section 5.2: a function this implementation does not know
        // is answered with -1 rather than left to fault the guest.
        _ => Action::Return(RET_NOT_SUPPORTED),
    }
}

/// Whether `function` is one this backend implements, for `PSCI_FEATURES`.
fn supported(function: u64) -> bool {
    matches!(
        function,
        FN_PSCI_VERSION
            | FN_MIGRATE_INFO_TYPE
            | FN_CPU_SUSPEND
            | FN64_CPU_SUSPEND
            | FN_CPU_OFF
            | FN_CPU_ON
            | FN64_CPU_ON
            | FN_AFFINITY_INFO
            | FN64_AFFINITY_INFO
            | FN_SYSTEM_OFF
            | FN_SYSTEM_RESET
            | FN_PSCI_FEATURES
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(running: &[bool]) -> Machine<'_> {
        Machine { running }
    }

    #[test]
    fn the_version_and_the_migration_calls_are_answered() {
        let m = machine(&[true]);
        assert_eq!(
            handle(FN_PSCI_VERSION, 0, 0, 0, &m),
            Action::Return(PSCI_VERSION_1_1)
        );
        assert_eq!(
            handle(FN_MIGRATE_INFO_TYPE, 0, 0, 0, &m),
            Action::Return(MIGRATE_INFO_TOS_MP)
        );
        assert_eq!(
            handle(FN_MIGRATE, 0, 0, 0, &m),
            Action::Return(RET_NOT_SUPPORTED)
        );
        assert_eq!(
            handle(FN_MIGRATE_INFO_UP_CPU, 0, 0, 0, &m),
            Action::Return(RET_NOT_SUPPORTED)
        );
    }

    #[test]
    fn an_unknown_function_returns_minus_one_rather_than_faulting_the_guest() {
        let m = machine(&[true]);
        for unknown in [0x8400_000b, 0xc400_0012, 0xdead_beef, 0] {
            assert_eq!(
                handle(unknown, 0, 0, 0, &m),
                Action::Return(RET_NOT_SUPPORTED),
                "function {unknown:#x}"
            );
        }
        assert_eq!(RET_NOT_SUPPORTED, u64::MAX, "X0 = -1");
    }

    #[test]
    fn a_secondary_vcpu_can_be_started_once() {
        let mut running = [true, false];
        let m = machine(&running);

        // Affinity 0 is the vCPU index; the entry point and context id are
        // passed through to the new vCPU.
        assert_eq!(
            handle(FN64_CPU_ON, 1, 0x4000_0000, 0x1234, &m),
            Action::CpuOn {
                mpidr: 1,
                entry: 0x4000_0000,
                context_id: 0x1234,
            }
        );

        // A vCPU that is already on, and one that does not exist.
        running[1] = true;
        let m = machine(&running);
        assert_eq!(
            handle(FN64_CPU_ON, 1, 0, 0, &m),
            Action::Return(RET_ALREADY_ON)
        );
        assert_eq!(
            handle(FN64_CPU_ON, 7, 0, 0, &m),
            Action::Return(RET_INVALID_PARAMS)
        );
        // An MPIDR in another cluster names no vCPU of this machine.
        assert_eq!(
            handle(FN64_CPU_ON, 0x100, 0, 0, &m),
            Action::Return(RET_INVALID_PARAMS)
        );

        // The 32-bit encoding is accepted as well (a 32-bit guest would send
        // it; the guest this port boots sends the 64-bit one).
        running[1] = false;
        let m = machine(&running);
        assert_eq!(
            handle(FN_CPU_ON, 1, 0x1000, 0, &m),
            Action::CpuOn {
                mpidr: 1,
                entry: 0x1000,
                context_id: 0
            }
        );
    }

    #[test]
    fn affinity_info_reports_whether_a_vcpu_is_on() {
        let running = [true, false];
        let m = machine(&running);
        for function in [FN_AFFINITY_INFO, FN64_AFFINITY_INFO] {
            assert_eq!(
                handle(function, 0, 0, 0, &m),
                Action::Return(AFFINITY_LEVEL_ON)
            );
            assert_eq!(
                handle(function, 1, 0, 0, &m),
                Action::Return(AFFINITY_LEVEL_OFF)
            );
            assert_eq!(
                handle(function, 3, 0, 0, &m),
                Action::Return(RET_INVALID_PARAMS),
                "no such vCPU"
            );
            // Levels above zero are always on.
            assert_eq!(
                handle(function, 1, 1, 0, &m),
                Action::Return(AFFINITY_LEVEL_ON)
            );
        }
    }

    #[test]
    fn the_power_down_calls_stop_what_they_say_they_stop() {
        let running = [true, false];
        let m = machine(&running);
        assert_eq!(handle(FN_CPU_OFF, 0, 0, 0, &m), Action::CpuOff);
        assert_eq!(
            handle(FN_SYSTEM_OFF, 0, 0, 0, &m),
            Action::SystemOff { reset: false }
        );
        assert_eq!(
            handle(FN_SYSTEM_RESET, 0, 0, 0, &m),
            Action::SystemOff { reset: true }
        );
    }

    #[test]
    fn suspend_only_accepts_a_power_state_this_machine_can_enter() {
        let running = [true];
        let m = machine(&running);
        for function in [FN_CPU_SUSPEND, FN64_CPU_SUSPEND] {
            assert_eq!(handle(function, 0, 0, 0, &m), Action::Suspend);
            // A shallow state whose identifier fits in the 16-bit field is
            // fine.
            assert_eq!(handle(function, 1, 0, 0, &m), Action::Suspend);
            // Anything above the state identifier is a state this machine
            // cannot enter.
            assert_eq!(
                handle(function, 0x0002_0000, 0, 0, &m),
                Action::Return(RET_INVALID_PARAMS)
            );
            // ... including an extended state (bit 30 marks the type).
            assert_eq!(
                handle(function, 1 << 30, 0, 0, &m),
                Action::Return(RET_INVALID_PARAMS)
            );
        }
    }

    #[test]
    fn features_answers_for_exactly_the_implemented_functions() {
        let m = machine(&[true, false]);
        for function in [
            FN_PSCI_VERSION,
            FN_MIGRATE_INFO_TYPE,
            FN_CPU_SUSPEND,
            FN64_CPU_SUSPEND,
            FN_CPU_OFF,
            FN_CPU_ON,
            FN64_CPU_ON,
            FN_AFFINITY_INFO,
            FN64_AFFINITY_INFO,
            FN_SYSTEM_OFF,
            FN_SYSTEM_RESET,
            FN_PSCI_FEATURES,
        ] {
            assert_eq!(
                handle(FN_PSCI_FEATURES, function, 0, 0, &m),
                Action::Return(RET_SUCCESS),
                "function {function:#x}"
            );
        }
        for function in [FN_MIGRATE, FN_MIGRATE_INFO_UP_CPU, 0x8400_000b, 0xc400_0012] {
            assert_eq!(
                handle(FN_PSCI_FEATURES, function, 0, 0, &m),
                Action::Return(RET_NOT_SUPPORTED),
                "function {function:#x}"
            );
        }
    }
}
