// Copyright 2020 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/arch/aarch64/gic/gicv3/regs/mod.rs
// (trimmed: everything ITS-related — `its_regs.rs`, the ITS tables flush and
// `ItsRegisterState` — belongs to the snapshot format, which this port does not
// have, so the state saved and restored here is the distributor, the
// redistributors and the CPU interface registers; the `/dev/kvm`-dependent
// tests are dropped, as elsewhere in this port).

mod dist_regs;
mod icc_regs;
mod redist_regs;

use kvm_ioctls::DeviceFd;

use crate::arch::aarch64::gic::GicError;
use crate::arch::aarch64::gic::regs::{GicState, GicVcpuState};

/// Save the state of the GIC device.
pub fn save_state(gic_device: &DeviceFd, mpidrs: &[u64]) -> Result<GicState, GicError> {
    // Flush redistributors pending tables to guest RAM.
    super::save_pending_tables(gic_device)?;

    let mut vcpu_states = Vec::with_capacity(mpidrs.len());
    for mpidr in mpidrs {
        vcpu_states.push(GicVcpuState {
            rdist: redist_regs::get_redist_regs(gic_device, *mpidr)?,
            icc: icc_regs::get_icc_regs(gic_device, *mpidr)?,
        })
    }

    Ok(GicState {
        dist: dist_regs::get_dist_regs(gic_device)?,
        gic_vcpu_states: vcpu_states,
    })
}

/// Restore the state of the GIC device.
pub fn restore_state(
    gic_device: &DeviceFd,
    mpidrs: &[u64],
    state: &GicState,
) -> Result<(), GicError> {
    dist_regs::set_dist_regs(gic_device, &state.dist)?;

    if mpidrs.len() != state.gic_vcpu_states.len() {
        return Err(GicError::InconsistentVcpuCount);
    }
    for (mpidr, vcpu_state) in mpidrs.iter().zip(&state.gic_vcpu_states) {
        redist_regs::set_redist_regs(gic_device, *mpidr, &vcpu_state.rdist)?;
        icc_regs::set_icc_regs(gic_device, *mpidr, &vcpu_state.icc)?;
    }

    Ok(())
}
