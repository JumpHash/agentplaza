// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: weft's git readers parse pack bodies, pack indexes and
// loose objects from bytes it did not write, so all three are fuzzed directly.

//! Fuzz target: weft's git object readers (`weft::git::fuzz_parse`).
//!
//! Every input byte is untrusted. Malformed input must come back as an error; a
//! panic, a hang or an unbounded allocation here is a bug in the parser.

#![cfg_attr(fuzzing, no_main)]

#[cfg(not(fuzzing))]
use std::process::ExitCode;

#[cfg(fuzzing)]
libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    weft::git::fuzz_parse(data);
});

/// Without LibFuzzer (`cargo check`/`cargo run`, no nightly toolchain): run the
/// parsers over a fixed input so the plumbing is exercised.
#[cfg(not(fuzzing))]
fn main() -> ExitCode {
    weft::git::fuzz_parse(b"PACK\0\0\0\x02\0\0\0\0");
    ExitCode::SUCCESS
}
