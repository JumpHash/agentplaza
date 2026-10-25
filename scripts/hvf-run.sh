#!/bin/sh
# Runs a binary that creates a Hypervisor.framework VM under the entitlement the framework checks
# for (`com.apple.security.hypervisor`, in scripts/hvf-entitlements.plist). Without it
# `hv_vm_create` fails with HV_DENIED.
#
# Cargo starts a test binary through `CARGO_TARGET_<TRIPLE>_RUNNER`, so the HVF tests of the `vmm`
# crate run under this script:
#
#   CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="$PWD/scripts/hvf-run.sh" cargo test -p vmm
#
# The variable is set for that one command; the owner's cargo configuration is not changed. The
# binary is signed ad hoc (`codesign -s -`), never with an identity, and then run with its
# arguments. A test that starts another binary itself (the per-VM process, `apz-vmm`) cannot go
# through cargo's runner, so it signs that binary first:
#
#   scripts/hvf-run.sh --sign target/debug/apz-vmm
#
# Darwin only: elsewhere the binary is run as it is.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
entitlements="$here/hvf-entitlements.plist"

sign() {
    if [ "$(uname -s)" = Darwin ]; then
        codesign -s - --entitlements "$entitlements" -f "$1"
    fi
}

if [ "${1:-}" = --sign ]; then
    [ $# -eq 2 ] || { echo "usage: hvf-run.sh --sign BINARY" >&2; exit 2; }
    sign "$2"
    exit 0
fi
[ $# -ge 1 ] || { echo "usage: hvf-run.sh BINARY [ARGS...]" >&2; exit 2; }
sign "$1"
exec "$@"
