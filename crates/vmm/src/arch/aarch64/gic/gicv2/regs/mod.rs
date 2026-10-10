// Copyright 2020 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/arch/aarch64/gic/gicv2/regs/mod.rs
// (trimmed: the `/dev/kvm`-dependent save/restore tests are dropped, as
// elsewhere in this port; the register model itself is kept verbatim).

mod dist_regs;
mod icc_regs;

use kvm_ioctls::DeviceFd;

use crate::arch::aarch64::gic::GicError;
use crate::arch::aarch64::gic::regs::{GicState, GicVcpuState};

/// Save the state of the GIC device.
pub fn save_state(fd: &DeviceFd, mpidrs: &[u64]) -> Result<GicState, GicError> {
    let mut vcpu_states = Vec::with_capacity(mpidrs.len());
    for mpidr in mpidrs {
        vcpu_states.push(GicVcpuState {
            rdist: Vec::new(),
            icc: icc_regs::get_icc_regs(fd, *mpidr)?,
        })
    }

    Ok(GicState {
        dist: dist_regs::get_dist_regs(fd)?,
        gic_vcpu_states: vcpu_states,
        ..Default::default()
    })
}

/// Restore the state of the GIC device.
pub fn restore_state(fd: &DeviceFd, mpidrs: &[u64], state: &GicState) -> Result<(), GicError> {
    dist_regs::set_dist_regs(fd, &state.dist)?;

    if mpidrs.len() != state.gic_vcpu_states.len() {
        return Err(GicError::InconsistentVcpuCount);
    }
    for (mpidr, vcpu_state) in mpidrs.iter().zip(&state.gic_vcpu_states) {
        icc_regs::set_icc_regs(fd, *mpidr, &vcpu_state.icc)?;
    }

    Ok(())
}
