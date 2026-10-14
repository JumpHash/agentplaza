// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Shared plumbing for the `vmm` integration tests: launching a *cross-compiled*
// sibling binary — a probe child, the `jailer_probe` binary, the `apz-vmm`
// binary — the way `cargo test` launched the test binary itself.
//
// On a native run there is nothing to add. Under a user-mode emulator cargo
// starts the test binary through `CARGO_TARGET_<TRIPLE>_RUNNER` (for this
// port's `aarch64-unknown-linux-gnu` gate, `qemu-aarch64 -L <sysroot>`), but a
// binary the *test* spawns is started by the kernel's `binfmt_misc` handler,
// which has no `-L` sysroot: the aarch64 child dies in the dynamic loader
// (`Could not open '/lib/ld-linux-aarch64.so.1'`) before it runs a line of its
// own, and every assertion about it fails for a reason that is not the jail's.
//
// Reading cargo's own runner and prefixing it keeps a spawned child on the
// same footing as the test that spawned it. This is plumbing, not a verdict:
// what the child then *does* is still the only thing that decides pass, skip
// or fail.
//
// Each test binary uses part of this module, so unused items are expected.
#![allow(dead_code)]

use std::fmt;

// The runner logic lives in `apz_os::runner`, shared with apid's tests: one implementation.
pub use apz_os::runner::command;

/// Writes `message` as a line to the run's stderr, where it shows **even on a
/// passing test**: `libtest` captures `println!`/`eprintln!` for tests that
/// pass, so a skip a reader has to be able to tell from a pass is written
/// straight to the file descriptor instead.
pub fn note(message: impl fmt::Display) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr(), "{message}");
}
