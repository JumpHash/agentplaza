// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the vsock packet parser is on the guest-controlled
// path (it reads the descriptor chain the guest published), so it is fuzzed
// with arbitrary chains, headers and payloads. Upstream Firecracker fuzzes the
// same parser with a similarly shaped target.

//! Fuzz target: the vsock packet parser (`VsockPacketTx`/`VsockPacketRx`).
//!
//! Every input byte is a guest's choice — the descriptor flags, the descriptor
//! lengths, the 44-byte header and the payload all come from the input — so this
//! covers the parser's whole gate: descriptor readability, chain length versus
//! the header's advertised `len`, the `MAX_PKT_BUF_SIZE` ceiling, and the
//! offset arithmetic of the payload accessors. A panic, a hang or an unbounded
//! allocation here is a bug in the parser, not in the harness.

#![cfg_attr(fuzzing, no_main)]

#[cfg(not(fuzzing))]
use std::process::ExitCode;

// The harness; see `fuzz/src/lib.rs`.
use agentplaza_fuzz::drive_packet;

#[cfg(fuzzing)]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    drive_packet(data);
});

/// Without LibFuzzer (`cargo check`/`cargo run`, no nightly toolchain): run the
/// harness over a fixed, well-formed input so the plumbing is exercised.
#[cfg(not(fuzzing))]
fn main() -> ExitCode {
    let mut seed = vec![0u8; 64];
    seed[8..12].copy_from_slice(&4096u32.to_le_bytes());
    seed[12..16].copy_from_slice(&1u32.to_le_bytes());
    seed[16..20].copy_from_slice(&16u32.to_le_bytes());
    drive_packet(&seed);
    ExitCode::SUCCESS
}
