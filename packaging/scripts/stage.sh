#!/usr/bin/env bash
# Lays out one platform's release files the same way for every packager (packaging/README.md).
#   stage.sh <target-triple> <cargo-out-dir> <stage-dir> <version>
# Runs on Linux, macOS and Windows (Git Bash).
set -euo pipefail

triple=$1 out=$2 stage=$3 version=$4
root=$(cd "$(dirname "$0")/../.." && pwd)

# The VM node (noded, vsockd, apz-vmm, vmctl) ships where a VM backend exists: Linux (KVM) and
# Apple Silicon (Hypervisor.framework). Windows and Intel Macs run the hub, the desktop app and
# the node link, and use nodes elsewhere.
case "$triple" in
  *windows*) exe=.exe; node=no ;;
  x86_64-apple-darwin) exe=; node=no ;;
  *) exe=; node=yes ;;
esac

rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/share/agentplaza"

bins=(apid apz-link agentplaza agentplaza-desktop)
[ "$node" = yes ] && bins+=(noded vsockd apz-vmm vmctl)
for b in "${bins[@]}"; do
  src="$out/$b$exe"
  [ -f "$src" ] || { echo "stage.sh: missing $src" >&2; exit 1; }
  cp "$src" "$stage/bin/"
done

share="$stage/share/agentplaza"
cp -R "$root/apps/web/dist" "$share/console"
mkdir -p "$share/roles" "$share/prompts" "$share/icons"
cp "$root/docs/agents/initial-roles.json" "$share/roles/"
cp -R "$root/docs/codex-draft/roles/." "$share/prompts/"
cp "$root/packaging/icons/"* "$share/icons/"
printf '%s\n' "$version" > "$share/VERSION"

echo "staged $version for $triple in $stage:"
ls -la "$stage/bin"
