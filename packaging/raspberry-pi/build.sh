#!/usr/bin/env bash
# Builds the AgentPlaza package for Raspberry Pi 4/5 (Raspberry Pi OS 64-bit,
# arm64): a .deb made with fpm, and a .tar.gz of the same root tree.
#
# usage: build.sh <stage-dir> <version> <out-dir>
#   <stage-dir>  the stage (packaging/README.md): bin/, share/agentplaza/
set -euo pipefail

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <stage-dir> <version> <out-dir>" >&2
    exit 2
fi

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
linux="$here/../linux"
stage="$(cd "$1" && pwd)"
version="$2"
out="$(mkdir -p "$3" && cd "$3" && pwd)"

command -v fpm >/dev/null 2>&1 || { echo "fpm is needed: gem install fpm" >&2; exit 1; }

for f in \
    "$stage/bin/noded" \
    "$linux/agentplaza.desktop" \
    "$linux/agentplaza-hub.service" \
    "$linux/agentplaza-node.service" \
    "$linux/postinstall.sh" \
    "$here/agentplaza-node.default" \
    "$here/postinstall.sh"; do
    [ -f "$f" ] || { echo "missing: $f" >&2; exit 1; }
done

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
root="$work/root"
mkdir -p "$root/usr/bin" "$root/usr/share/agentplaza"
cp -a "$stage/bin/." "$root/usr/bin/"
cp -a "$stage/share/agentplaza/." "$root/usr/share/agentplaza/"
for n in 16 24 32 48 64 128 256 512; do
    install -Dm644 "$stage/share/agentplaza/icons/agentplaza-$n.png" \
        "$root/usr/share/icons/hicolor/${n}x${n}/apps/agentplaza.png"
done

install -Dm644 "$linux/agentplaza.desktop" "$root/usr/share/applications/agentplaza.desktop"
# AppStream metadata, as in the Linux packages: the software installers show it as an app.
sed -e "s/@VERSION@/$version/" -e "s/@DATE@/$(date -u +%F)/" "$linux/io.agentplaza.AgentPlaza.metainfo.xml" \
    | install -Dm644 /dev/stdin "$root/usr/share/metainfo/io.agentplaza.AgentPlaza.metainfo.xml"
mkdir -p "$root/usr/lib/systemd/user"
install -m644 "$linux"/*.service "$root/usr/lib/systemd/user/"
install -Dm644 "$here/agentplaza-node.default" "$root/etc/default/agentplaza-node"
install -Dm755 "$linux/postinstall.sh" "$root/usr/share/agentplaza/linux-postinstall.sh"

deb="$out/agentplaza_${version}_arm64-raspberrypi.deb"
fpm -s dir -t deb \
    -C "$root" \
    --name agentplaza \
    --version "$version" \
    --architecture arm64 \
    --package "$deb" \
    --after-install "$here/postinstall.sh" \
    --config-files /etc/default/agentplaza-node \
    --deb-recommends docker.io \
    --license Apache-2.0 \
    --url https://github.com/JumpHash/agentplaza \
    --maintainer "JUMPHASH <hello@jumphash.com>" \
    --description "AgentPlaza for Raspberry Pi 4/5 (64-bit)" \
    --vendor JUMPHASH \
    --category devel \
    --deb-priority optional \
    .

tarball="$out/agentplaza-${version}-raspberrypi-arm64.tar.gz"
tar --numeric-owner --owner=0 --group=0 -C "$root" -czf "$tarball" .

echo "built:"
ls -l "$deb" "$tarball"
