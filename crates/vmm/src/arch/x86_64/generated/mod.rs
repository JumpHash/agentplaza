// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/arch/x86_64/generated/mod.rs
// (trimmed: hyperv, hyperv_tlfs and perf_event only feed the snapshot MSR
// dump lists, which are not ported).

pub mod arch_prctl;
pub mod mpspec;
pub mod msr_index;
