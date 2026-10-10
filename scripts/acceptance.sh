#!/usr/bin/env bash
# AgentPlaza acceptance suites — the four fixed suites, runnable from CI.
#
#   scripts/acceptance.sh [demo|redteam|killresume|budgets|all] [run.py options]
#
# This is a thin wrapper around tests/acceptance/run.py (the repository's
# convention: a shell entry point per test tree, docs/*/check.sh). It needs
# only python3 and the built binaries (target/debug/noded, target/debug/vsockd)
# plus the guest images (target/guest/initrd.x86_64.gz), so build the workspace
# first:
#
#   cargo build --workspace && scripts/mk-initrd-x86_64.sh
#
# Exit status: 0 all executed suites passed, 1 a suite failed, 2 blocked.
set -euo pipefail

cd "$(dirname "$0")/.."
exec python3 -I tests/acceptance/run.py "$@"
