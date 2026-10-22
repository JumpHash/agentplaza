#!/usr/bin/env bash
# Lays out one platform's release files the same way for every packager (packaging/README.md).
#   stage.sh <target-triple> <cargo-out-dir> <stage-dir> <version>
# Runs on Linux, macOS and Windows (Git Bash).
set -euo pipefail

triple=$1 out=$2 stage=$3 version=$4
root=$(cd "$(dirname "$0")/../.." && pwd)

# The VM node ships on Linux (KVM), Apple Silicon (Hypervisor.framework) and Windows x64 (Windows
# Hypervisor Platform). apz-vmm, the per-VM process, is not built for Windows: Windows x64 gets
# noded, vsockd and vmctl. Windows arm64 runs the hub, the desktop app and the CLI (its VM node is in
# development, docs/windows-whp-arm64.md); Intel Macs run the hub and the desktop app and use nodes
# elsewhere.
case "$triple" in
  x86_64-pc-windows-msvc) exe=.exe; node=yes; node_bins=(noded vsockd vmctl) ;;
  *windows*) exe=.exe; node=no; node_bins=() ;;
  x86_64-apple-darwin) exe=; node=no; node_bins=() ;;
  *) exe=; node=yes; node_bins=(noded vsockd apz-vmm vmctl) ;;
esac

rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/share/agentplaza"

bins=(apid apz-link agentplaza agentplaza-desktop)
[ "$node" = yes ] && bins+=("${node_bins[@]}")
for b in "${bins[@]}"; do
  src="$out/$b$exe"
  [ -f "$src" ] || { echo "stage.sh: missing $src" >&2; exit 1; }
  cp "$src" "$stage/bin/"
done

# The console ships with its wasm engine and the IDE's wasm (apps/web/build.mjs): a dist without them has no 3D
# or no IDE, so refuse to stage it.
for pat in 'apz_engine_web-*_bg.wasm' 'apz_engine_web-*.js' 'apz_ide_web-*_bg.wasm' 'apz_ide_web-*.js'; do
  compgen -G "$root/apps/web/dist/$pat" >/dev/null \
    || { echo "stage.sh: apps/web/dist has no $pat: rebuild the console with the engine (apps/web/build.mjs)" >&2; exit 1; }
done

share="$stage/share/agentplaza"
cp -R "$root/apps/web/dist" "$share/console"
mkdir -p "$share/roles" "$share/prompts" "$share/icons"
cp "$root/docs/agents/initial-roles.json" "$share/roles/"
cp -R "$root/docs/codex-draft/roles/." "$share/prompts/"
cp "$root/packaging/icons/"* "$share/icons/"
printf '%s\n' "$version" > "$share/VERSION"

# The agents' VM kernel and initrd (scripts/mk-guest.sh, built per architecture by guest.yml), found
# by noded. Only where the stage has the VM node; a missing guest fails unless APZ_ALLOW_NO_GUEST=1.
if [ "$node" = yes ]; then
  case "$triple" in
    x86_64-unknown-linux-gnu|x86_64-pc-windows-msvc) garch=x86_64 ;;
    aarch64-unknown-linux-gnu|aarch64-apple-darwin) garch=aarch64 ;;
    *) echo "stage.sh: no guest image for $triple" >&2; exit 1 ;;
  esac
  gsrc="${APZ_GUEST_DIR:-$root/target/guest}/$garch"
  missing=
  for f in vmlinux initrd.gz guest.json; do
    [ -s "$gsrc/$f" ] || missing="$missing $gsrc/$f"
  done
  if [ -z "$missing" ]; then
    mkdir -p "$share/guest/$garch"
    cp "$gsrc/vmlinux" "$gsrc/initrd.gz" "$gsrc/guest.json" "$share/guest/$garch/"
  elif [ "${APZ_ALLOW_NO_GUEST:-0}" = 1 ]; then
    echo "stage.sh: warning: no guest image, agents cannot run:$missing" >&2
  else
    echo "stage.sh: missing guest image:$missing (APZ_ALLOW_NO_GUEST=1 stages without it)" >&2
    exit 1
  fi
fi

echo "staged $version for $triple in $stage:"
ls -la "$stage/bin"
