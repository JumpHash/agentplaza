// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: a deterministic stand-in for a fuzzing run, so the
// harnesses are exercised by `cargo test` even where `cargo-fuzz` (nightly) is
// not installed.

//! A deterministic smoke run of both fuzz harnesses.
//!
//! `cargo fuzz` is nightly-only; this test runs the same harness bodies over a
//! few thousand pseudo-random inputs (xorshift64, fixed seed, input lengths from
//! empty to a full packet buffer) on any toolchain. A panic in the parser, the
//! state machine or the harness fails it, which is the property the targets
//! check — this is not a replacement for the targets, only a portable proof that
//! they are wired to something that runs.

use agentplaza_fuzz::{drive_csm, drive_packet};

/// xorshift64: deterministic, dependency-free, good enough to smear input bytes.
struct XorShift64(u64);

impl XorShift64 {
    fn next_byte(&mut self) -> u8 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 24) & 0xff) as u8
    }

    fn fill(&mut self, buf: &mut [u8]) {
        for byte in buf.iter_mut() {
            *byte = self.next_byte();
        }
    }
}

const INPUT_LENGTHS: [usize; 10] = [0, 1, 7, 31, 32, 33, 45, 64, 1024, 4096];

#[test]
fn the_packet_harness_survives_pseudo_random_inputs() {
    let mut rng = XorShift64(0x243f_6a88_85a3_08d3);
    for len in INPUT_LENGTHS {
        for _ in 0..256 {
            let mut buf = vec![0u8; len];
            rng.fill(&mut buf);
            drive_packet(&buf);
        }
    }
}

#[test]
fn the_csm_harness_survives_pseudo_random_inputs() {
    let mut rng = XorShift64(0x9e37_79b9_7f4a_7c15);
    for len in INPUT_LENGTHS {
        for _ in 0..256 {
            let mut buf = vec![0u8; len];
            rng.fill(&mut buf);
            drive_csm(&buf);
        }
    }
}
