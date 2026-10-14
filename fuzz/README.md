# vmm guest-parser fuzz targets

`cargo-fuzz` (LibFuzzer) targets for the parsers that read data the guest
controls in the virtio devices (vsock and block):

| target | drives |
|---|---|
| `vsock_packet` | the packet parser (`VsockPacketTx`/`VsockPacketRx`), i.e. descriptor-chain walking, the 44-byte header, and the payload accessors' offset arithmetic |
| `vsock_csm` | the connection state machine (`VsockConnection`), driven by arbitrary packet headers — op, shutdown flags, advertised lengths, flow-control credits — over a real socket pair |
| `block_request` | the virtio-block request parser (`Request::parse`), i.e. the descriptor chain's shape and directions, the 16-byte header (request type and sector), the length, direction, top-sector and device-id bounds, and the discard range table |

All three live in `fuzz/src/lib.rs` (`drive_packet`, `drive_csm`,
`drive_block_request`) so that `tests/smoke.rs` can run the same bodies over a
few thousand deterministic pseudo-random inputs on any toolchain; the binaries
in `fuzz_targets/` are thin LibFuzzer wrappers.

`drive_block_request` returns a `BlockOutcome` (`Parsed`, `Rejected` with the gate that refused
it and the request type the header declared, or `Unbuilt`) and `tests/smoke.rs` tallies those into a `BlockCoverage`: the smoke
run prints how many of its 5120 inputs reached a real parse, which parser gate
rejected the rest, and how many inputs the parser saw of each request type.
That is the coverage measure for this target — an input the parser accepted is
the only proof it walked past every gate, so a corpus that never parses is a
regression in the harness, not a pass.

This crate is **not** a member of the parent workspace (it has its own
`[workspace]` table): it needs the nightly-only fuzzing toolchain, and it must
not slow down or gate an ordinary build.

## Provenance

Written for this port (Copyright 2026 AgentPlaza, Apache-2.0). Upstream
Firecracker ships a `vsock_packet_fuzzer` and a `block` fuzzer of its own, but
this port's packet API and descriptor-chain layout differ and its block request
API is trimmed (`Request::parse` is the whole input surface), so nothing is
copied here; the target names and the shape of the harnesses follow upstream's
layout for familiarity.

## Running

The toolchain must be nightly, and `cargo-fuzz` must be installed:

```
cargo install cargo-fuzz
cargo +nightly fuzz run vsock_packet --features fuzzing
cargo +nightly fuzz run vsock_csm --features fuzzing
cargo +nightly fuzz run block_request --features fuzzing
```

`cargo fuzz` passes `--cfg fuzzing`, which selects the LibFuzzer entry point;
the `fuzzing` feature adds the `libfuzzer-sys` dependency that entry point needs
(it is optional so that a plain `cargo check` never needs it).

A corpus grows from real runs under `fuzz/corpus/<target>/` (not committed);
seed it from a live VM's traffic if a regression needs replaying. Per-target time
budgets belong in CI, not here; no workflow in `.github/` runs the targets yet.

## Without cargo-fuzz

`cargo build` is enough to prove they compile on any toolchain, and
`cargo run --bin vsock_packet` / `cargo run --bin vsock_csm` /
`cargo run --bin block_request` run the harnesses once over a fixed input (the
block target's fixed input is the well-formed read request, so a non-zero exit
means the harness or the parser regressed). `cargo test` runs the deterministic
smoke test, and the block one prints its coverage:

```
cargo check --all-targets     # any toolchain, no LibFuzzer
cargo test                    # the harnesses over ~10k pseudo-random inputs
cargo test -- --nocapture     # ... with the block coverage report
```

A panic in any harness — in the parser, the state machine, or the harness
itself — is a bug: it gets a regression test, not a suppression.
