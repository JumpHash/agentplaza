// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the vsock connection state machine (CSM) is driven
// entirely by guest packets — op, flags, advertised lengths and flow-control
// credits — so it is fuzzed with arbitrary headers and payloads over a real
// socket pair. Upstream Firecracker's fuzz directory has no CSM target; this one
// is written against this port's `VsockConnection` API.

//! Fuzz target: the vsock connection state machine.
//!
//! The guest chooses the op, the shutdown flags, the advertised packet length
//! and the flow-control credits. Those drive the CSM's arithmetic (credit
//! accounting, the TX buffer and its flush, the kill timer) and its state
//! transitions, so every state must be reachable without a panic. The host-side
//! stream is a socket pair this process owns, so the target exercises the real
//! read and write paths.

#![cfg_attr(fuzzing, no_main)]

#[cfg(not(fuzzing))]
use std::process::ExitCode;

// The harness; see `fuzz/src/lib.rs`.
use agentplaza_fuzz::drive_csm;

#[cfg(fuzzing)]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    drive_csm(data);
});

/// Without LibFuzzer (`cargo check`/`cargo run`, no nightly toolchain): run the
/// harness over a fixed input so the plumbing is exercised.
#[cfg(not(fuzzing))]
fn main() -> ExitCode {
    let mut seed = vec![0u8; 64];
    seed[8..12].copy_from_slice(&4096u32.to_le_bytes());
    drive_csm(&seed);
    ExitCode::SUCCESS
}
