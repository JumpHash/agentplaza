#!/usr/bin/env bash
# AgentPlaza acceptance suites — the four fixed suites plus the fleet, jail,
# treasury, social, agora and council suites, runnable from CI.
#
#   scripts/acceptance.sh [demo|redteam|killresume|budgets|fleet|jail|treasury|social|agora|council|all] [run.py options]
#
# This is a thin wrapper around tests/acceptance/run.py (the repository's
# convention: a shell entry point per test tree, docs/*/check.sh). It needs
# only python3 and the built binaries (target/debug/noded, target/debug/vsockd)
# plus the guest images (target/guest/initrd.x86_64.gz), so build the workspace
# first:
#
#   cargo build --workspace && scripts/mk-initrd-x86_64.sh
#
# The budgets suite's `--workspace` cells additionally need the base image of
# the guests' writable workspace volume (target/guest/workspace.ext4); the
# suite builds it with scripts/mk-workspace-image.sh when it is absent and
# reports those cells BLOCKED, with the reason, when mkfs.ext4 is not
# installed. Override the path with --workspace-image.
#
# Every daemon a suite spawns comes from --bin-dir (default target/debug), a
# directory every agent shares: a concurrent `cargo build` that relinks
# noded/vsockd mid-run makes the later suites test a different binary than the
# earlier ones. The runner records the binaries' digests before and after the
# suites and reports such a run as a FAILURE (never a mixture that reads as a
# PASS); for a hermetic run, pin the built binaries in a directory of their own
# and pass --bin-dir:
#
#   mkdir -p /tmp/apz-pin && cp target/debug/noded target/debug/vsockd /tmp/apz-pin/
#   scripts/acceptance.sh all --bin-dir /tmp/apz-pin
#
# Exit status: 0 all executed suites passed, 1 a suite failed, 2 blocked.
#
# --artifacts names this run's work directory and must be fresh (omit it for a
# mktemp -d): a non-empty one is refused, because a previous run's daemon logs
# and socket files satisfy the harness's readiness waits and the run would then
# report a mixture of two runs' state. --keep keeps the tree for inspection.
set -euo pipefail

cd "$(dirname "$0")/.."
exec python3 -I tests/acceptance/run.py "$@"
