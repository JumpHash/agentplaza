#!/usr/bin/env bash
# Build the Linux packages (deb, pacman, rpm), the AppImage and the tarball from a stage dir.
# Usage: build.sh <stage-dir> <version> <x86_64|aarch64> <out-dir>
# Needs fpm on PATH; rpm also needs rpmbuild; pacman needs bsdtar and zstd.
set -euo pipefail

if [ "$#" -ne 4 ]; then
  echo "usage: $0 <stage-dir> <version> <x86_64|aarch64> <out-dir>" >&2
  exit 2
fi
STAGE=$1
VERSION=$2
ARCH=$3
OUT=$4

case "$ARCH" in
  x86_64) DEB_ARCH=amd64 ;;
  aarch64) DEB_ARCH=arm64 ;;
  *) echo "unknown arch: $ARCH (want x86_64 or aarch64)" >&2; exit 2 ;;
esac

[ -d "$STAGE/bin" ] || { echo "missing $STAGE/bin" >&2; exit 2; }
[ -d "$STAGE/share/agentplaza/icons" ] || { echo "missing $STAGE/share/agentplaza/icons" >&2; exit 2; }
command -v fpm >/dev/null 2>&1 || { echo "fpm not found on PATH" >&2; exit 1; }

STAGE=$(cd "$STAGE" && pwd)
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
TREE="$WORK/root"

# Binaries and shared data.
install -d "$TREE/usr/bin" "$TREE/usr/share/agentplaza"
cp -a "$STAGE/bin/." "$TREE/usr/bin/"
cp -a "$STAGE/share/agentplaza/." "$TREE/usr/share/agentplaza/"

# Desktop entry and hicolor icons.
install -Dm644 "$HERE/agentplaza.desktop" "$TREE/usr/share/applications/agentplaza.desktop"
for n in 16 24 32 48 64 128 256 512; do
  install -Dm644 "$STAGE/share/agentplaza/icons/agentplaza-$n.png" \
    "$TREE/usr/share/icons/hicolor/${n}x${n}/apps/agentplaza.png"
done

# systemd user units.
install -Dm644 "$HERE/agentplaza-hub.service" "$TREE/usr/lib/systemd/user/agentplaza-hub.service"
install -Dm644 "$HERE/agentplaza-node.service" "$TREE/usr/lib/systemd/user/agentplaza-node.service"

# Short readme.
install -d "$TREE/usr/share/doc/agentplaza"
cat > "$TREE/usr/share/doc/agentplaza/README" <<'EOF'
AgentPlaza
==========

Installed:
  /usr/bin/apid                 hub (the town server)
  /usr/bin/noded                node (runs agents in VMs)
  /usr/bin/agentplaza-desktop   desktop app
  /usr/lib/systemd/user/        agentplaza-hub.service, agentplaza-node.service
  /usr/share/agentplaza/        shared data and icons

Start the hub as your user:
  systemctl --user enable --now agentplaza-hub

Then open AgentPlaza from your application menu, or run agentplaza-desktop.

KVM: agents need /dev/kvm. Enable virtualization (Intel VT-x or AMD-V) in
the firmware. On a Raspberry Pi use the 64-bit OS.

Optional settings, read by the units if present:
  ~/.config/agentplaza/hub.env
  ~/.config/agentplaza/node.env
  /etc/default/agentplaza-node
EOF
chmod 0644 "$TREE/usr/share/doc/agentplaza/README"

DESC="AgentPlaza: a town for your AI agents (hub, node and desktop app)"
FPM_COMMON=(
  -s dir -C "$TREE"
  --name agentplaza
  --version "$VERSION"
  --license Apache-2.0
  --url https://github.com/JumpHash/agentplaza-mvp
  --maintainer "JUMPHASH <hello@jumphash.com>"
  --description "$DESC"
  --after-install "$HERE/postinstall.sh"
)

# fpm refuses to overwrite, so clear this version's earlier outputs first.
rm -f "$OUT"/agentplaza_"$VERSION"_* "$OUT"/agentplaza-"$VERSION"-*

DEB="$OUT/agentplaza_${VERSION}_${DEB_ARCH}.deb"
fpm "${FPM_COMMON[@]}" -t deb --architecture "$DEB_ARCH" \
  --deb-recommends docker.io --package "$DEB" .
echo "built: $DEB"

PAC="$OUT/agentplaza-${VERSION}-1-${ARCH}.pkg.tar.zst"
fpm "${FPM_COMMON[@]}" -t pacman --iteration 1 --architecture "$ARCH" \
  --pacman-compression zstd --package "$PAC" .
echo "built: $PAC"

TGZ="$OUT/agentplaza-${VERSION}-linux-${ARCH}.tar.gz"
cp -a "$STAGE" "$WORK/agentplaza-$VERSION"
tar -C "$WORK" -czf "$TGZ" "agentplaza-$VERSION"
echo "built: $TGZ"

# The AppImage: one file that runs on any distribution (glibc 2.35 or newer) by a double-click,
# with its icon, and is the command line tool when run through a link named `agentplaza` (AppRun).
# Built with appimagetool and the static type2 runtime for $ARCH (the CI image has both:
# $APZ_APPIMAGE_RUNTIMES); skipped, with a note, where they are missing.
APPIMAGE="$OUT/AgentPlaza-${VERSION}-linux-${ARCH}.AppImage"
RUNTIME="${APZ_APPIMAGE_RUNTIMES:-/opt/appimage-runtime}/runtime-$ARCH"
if command -v appimagetool >/dev/null 2>&1 && [ -f "$RUNTIME" ]; then
  APPDIR="$WORK/AgentPlaza.AppDir"
  install -d "$APPDIR/usr"
  cp -a "$TREE/usr/bin" "$TREE/usr/share" "$APPDIR/usr/"
  install -m755 "$HERE/AppRun" "$APPDIR/AppRun"
  install -m644 "$HERE/agentplaza.desktop" "$APPDIR/agentplaza.desktop"
  install -m644 "$STAGE/share/agentplaza/icons/agentplaza-256.png" "$APPDIR/agentplaza.png"
  ln -s agentplaza.png "$APPDIR/.DirIcon"
  rm -f "$APPIMAGE"
  ARCH=$ARCH appimagetool --no-appstream --runtime-file "$RUNTIME" "$APPDIR" "$APPIMAGE" >/dev/null
  echo "built: $APPIMAGE"
else
  echo "note: no appimagetool or $RUNTIME here; skipping the AppImage" >&2
fi

# rpm last, so a host without rpmbuild still gets the other artifacts.
RPM="$OUT/agentplaza-${VERSION}-1.${ARCH}.rpm"
fpm "${FPM_COMMON[@]}" -t rpm --iteration 1 --architecture "$ARCH" --package "$RPM" .
echo "built: $RPM"
