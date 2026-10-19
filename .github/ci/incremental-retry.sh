#!/usr/bin/env bash
# incremental-retry.sh <target triple> <command...>: runs a `checked` build or test command, and when the build itself
# failed (exit 101; nextest gives 100 for failing tests, which are never retried) runs it once more without the
# incremental cache. The runners keep cargo's build directory between runs, and an incremental optimised build can
# reuse a stale codegen unit, so the link fails with undefined `….llvm.<n>` symbols that a clean build of the crate
# does not have.
set -u
triple=$1
shift
"$@"
code=$?
[ "$code" -eq 101 ] || exit "$code"
cache="$CARGO_TARGET_DIR/$triple/checked/incremental"
echo "::warning::the build failed; trying once more without the incremental cache ($cache)"
rm -rf "$cache"
"$@"
