#!/usr/bin/env bash
# AgentPlaza acceptance suites — the four fixed suites plus the fleet and
# treasury suites, runnable from CI.
#
#   scripts/acceptance.sh [demo|redteam|killresume|budgets|fleet|treasury|all] [run.py options]
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
# Exit status: 0 all executed suites passed, 1 a suite failed, 2 blocked.
set -euo pipefail

cd "$(dirname "$0")/.."
exec python3 -I tests/acceptance/run.py "$@"
