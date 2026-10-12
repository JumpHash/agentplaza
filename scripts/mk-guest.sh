#!/usr/bin/env bash
# Builds the guest the agents' VMs boot, for one architecture: the kernel
# (scripts/mk-guest-kernel.sh, every driver built in) and the initramfs (scripts/mk-initrd.sh
# with APZ_GUEST_MODULES=builtin), into the layout every package ships and noded finds:
#
#   target/guest/<arch>/vmlinux     the kernel (x86_64: bzImage; aarch64: Image)
#   target/guest/<arch>/initrd.gz   the initramfs (PID 1, the agent harness, busybox, Alpine)
#   target/guest/<arch>/guest.json  versions and SHA-256s
#
#   scripts/mk-guest.sh <x86_64|aarch64>
#
# Native builds need gcc, make, flex, bison, bc, libelf, openssl headers, cpio, xz, curl, a
# static busybox (Debian/Ubuntu: busybox-static) and Rust. Building aarch64 on x86_64 uses
# aarch64-linux-gnu-gcc (and its static C library) when it is installed.
set -euo pipefail
cd "$(dirname "$0")/.."

arch=${1:?usage: mk-guest.sh <x86_64|aarch64>}
host=$(uname -m)
[ "$host" = arm64 ] && host=aarch64
if [ "$arch" != "$host" ] && [ -z "${CROSS_COMPILE:-}" ]; then
  if command -v "$arch-linux-gnu-gcc" >/dev/null 2>&1; then
    export CROSS_COMPILE="$arch-linux-gnu-"
    upper=$(echo "$arch" | tr '[:lower:]' '[:upper:]')
    export "CARGO_TARGET_${upper}_UNKNOWN_LINUX_GNU_LINKER=$arch-linux-gnu-gcc"
  else
    echo "mk-guest: building $arch on $host needs $arch-linux-gnu-gcc (or CROSS_COMPILE)" >&2
    exit 1
  fi
fi

out="target/guest/$arch"
mkdir -p "$out"
scripts/mk-guest-kernel.sh "$arch" "$out/vmlinux"
APZ_GUEST_MODULES=builtin sh scripts/mk-initrd.sh "$arch"
cp "target/guest/initrd.$arch.gz" "$out/initrd.gz"

sha() { sha256sum "$1" | cut -d' ' -f1; }
cat > "$out/guest.json" <<EOF
{
  "arch": "$arch",
  "kernel": { "version": "$(cat "$out/kernel.version")", "file": "vmlinux", "sha256": "$(sha "$out/vmlinux")" },
  "initrd": { "file": "initrd.gz", "sha256": "$(sha "$out/initrd.gz")" },
  "source": "$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
}
EOF
cat "$out/guest.json"
