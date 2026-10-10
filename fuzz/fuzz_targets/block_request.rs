// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the virtio-block request parser is on the
// guest-controlled path (it reads the descriptor chain the guest published and
// the request header and discard range the guest wrote), so it is fuzzed with
// arbitrary chains, headers and ranges. Upstream Firecracker fuzzes the same
// parser with a similarly shaped `block` target.

//! Fuzz target: the virtio-block request parser (`Request::parse`).
//!
//! Every input byte is a guest's choice — the descriptor chain's shape, each
//! descriptor's address, direction and length, the `next` index, the request
//! type and sector in the 16-byte header, and the discard range table. That
//! covers the parser's whole gate: the head/data/status direction rules, the
//! status length minimum, the 512-multiple data-length check, the top-sector
//! bound (including the `checked_add` overflow at `u64::MAX`), the discard
//! range bound and its flags, the device-id length minimum, and the malformed
//! and short chains. A panic, a hang or an unbounded allocation here is a bug
//! in the parser, not in the harness.

#![cfg_attr(fuzzing, no_main)]

#[cfg(not(fuzzing))]
use std::process::ExitCode;

// The harness; see `fuzz/src/lib.rs`.
use agentplaza_fuzz::drive_block_request;

// Only the fixed-input `main` (the non-LibFuzzer build) inspects the outcome.
#[cfg(not(fuzzing))]
use agentplaza_fuzz::BlockOutcome;

#[cfg(fuzzing)]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    drive_block_request(data);
});

/// Without LibFuzzer (`cargo check`/`cargo run`, no nightly toolchain): run the
/// harness over a fixed input so the plumbing is exercised. An all-zero input
/// maps to the well-formed read request, so a non-zero exit here means the
/// harness or the parser regressed.
#[cfg(not(fuzzing))]
fn main() -> ExitCode {
    let seed = vec![0u8; 64];
    match drive_block_request(&seed) {
        BlockOutcome::Parsed(_) => ExitCode::SUCCESS,
        other => {
            eprintln!("block_request: the fixed input did not parse: {other:?}");
            ExitCode::FAILURE
        }
    }
}
