# vmm vsock fuzz targets

`cargo-fuzz` (LibFuzzer) targets for the parsers that read data the guest
controls in the virtio-vsock device:

| target | drives |
|---|---|
| `vsock_packet` | the packet parser (`VsockPacketTx`/`VsockPacketRx`), i.e. descriptor-chain walking, the 44-byte header, and the payload accessors' offset arithmetic |
| `vsock_csm` | the connection state machine (`VsockConnection`), driven by arbitrary packet headers — op, shutdown flags, advertised lengths, flow-control credits — over a real socket pair |

Both live in `fuzz/src/lib.rs` (`drive_packet`, `drive_csm`) so that
`tests/smoke.rs` can run the same bodies over a few thousand deterministic
pseudo-random inputs on any toolchain; the binaries in `fuzz_targets/` are thin
LibFuzzer wrappers.

This crate is **not** a member of the parent workspace (it has its own
`[workspace]` table): it needs the nightly-only fuzzing toolchain, and it must
not slow down or gate an ordinary build.

## Provenance

Written for this port (Copyright 2026 AgentPlaza, Apache-2.0). Upstream
Firecracker ships a `vsock_packet_fuzzer` of its own, but this port's packet API
and descriptor-chain layout differ, so nothing is copied here; the target names
and the shape of the harness follow upstream's layout for familiarity.

## Running

The toolchain must be nightly, and `cargo-fuzz` must be installed:

```
cargo install cargo-fuzz
cargo +nightly fuzz run vsock_packet --features fuzzing
cargo +nightly fuzz run vsock_csm --features fuzzing
```

`cargo fuzz` passes `--cfg fuzzing`, which selects the LibFuzzer entry point;
the `fuzzing` feature adds the `libfuzzer-sys` dependency that entry point needs
(it is optional so that a plain `cargo check` never needs it).

A corpus grows from real runs under `fuzz/corpus/<target>/` (not committed);
seed it from a live VM's traffic if a regression needs replaying. Per-target time
budgets belong in CI, not here.

## Without cargo-fuzz

`cargo build` is enough to prove they compile on any toolchain, and
`cargo run --bin vsock_packet` / `cargo run --bin vsock_csm` run the harnesses
once over a fixed input. `cargo test` runs the deterministic smoke test:

```
cargo check --all-targets     # any toolchain, no LibFuzzer
cargo test                    # the harnesses over ~10k pseudo-random inputs
```

A panic in either harness — in the parser, the state machine, or the harness
itself — is a bug: it gets a regression test, not a suppression.
