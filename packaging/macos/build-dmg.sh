#!/usr/bin/env bash
# Builds a universal AgentPlaza.app from an arm64 and an x86_64 stage, signs it,
# and wraps it in a DMG. Ad-hoc signed unless APPLE_SIGNING_IDENTITY is set.
#
# Usage: build-dmg.sh <stage-arm64> <stage-x86_64> <version> <out-dir>
#
# Each stage has bin/ (binaries) and share/agentplaza/ (data and icons).
#
# Environment:
#   APPLE_SIGNING_IDENTITY  Developer ID Application identity; enables timestamped signing.
#   APPLE_NOTARY_PROFILE    notarytool keychain profile; notarizes and staples the DMG.
#                           Requires APPLE_SIGNING_IDENTITY.
set -euo pipefail

die() { echo "build-dmg: $*" >&2; exit 1; }
warn() { echo "build-dmg: warning: $*" >&2; }

if [ "$#" -ne 4 ]; then
  die "usage: $0 <stage-arm64> <stage-x86_64> <version> <out-dir>"
fi
ARM_STAGE="$1"
X86_STAGE="$2"
VERSION="$3"
OUT_DIR="$4"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

case "$VERSION" in
  ""|*[!0-9A-Za-z.+-]*) die "invalid version: $VERSION" ;;
esac
[ -d "$ARM_STAGE/bin" ] || die "missing $ARM_STAGE/bin"
[ -d "$X86_STAGE/bin" ] || die "missing $X86_STAGE/bin"
[ -d "$ARM_STAGE/share/agentplaza" ] || die "missing $ARM_STAGE/share/agentplaza"
ICON="$ARM_STAGE/share/agentplaza/icons/agentplaza.icns"
[ -f "$ICON" ] || die "missing $ICON"

if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  SIGN_ID="$APPLE_SIGNING_IDENTITY"
  TS_FLAG=(--timestamp)
else
  SIGN_ID="-"
  TS_FLAG=(--timestamp=none)
fi
if [ -n "${APPLE_NOTARY_PROFILE:-}" ] && [ "$SIGN_ID" = "-" ]; then
  die "APPLE_NOTARY_PROFILE needs APPLE_SIGNING_IDENTITY (notarization requires a Developer ID signature)"
fi

sign() {
  codesign --force --options runtime "${TS_FLAG[@]}" -s "$SIGN_ID" "$@"
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

DMG_SRC="$WORK/dmg"
APP="$DMG_SRC/AgentPlaza.app"
MACOS="$APP/Contents/MacOS"
mkdir -p "$MACOS" "$APP/Contents/Resources/share"

# Universal binaries: one lipo per name present in both stages.
NAMES="$(printf '%s\n%s\n' "$(ls -1 "$ARM_STAGE/bin")" "$(ls -1 "$X86_STAGE/bin")" | sort -u | sed '/^$/d')"
[ -n "$NAMES" ] || die "no binaries in the stages"
while IFS= read -r name; do
  arm="$ARM_STAGE/bin/$name"
  x86="$X86_STAGE/bin/$name"
  out="$MACOS/$name"
  if [ -f "$arm" ] && [ -f "$x86" ]; then
    lipo -create "$arm" "$x86" -output "$out"
  elif [ -f "$arm" ]; then
    warn "$name is only in the arm64 stage; copying it as is"
    cp "$arm" "$out"
  else
    warn "$name is only in the x86_64 stage; copying it as is"
    cp "$x86" "$out"
  fi
  chmod 755 "$out"
done <<< "$NAMES"

cp -R "$ARM_STAGE/share/agentplaza" "$APP/Contents/Resources/share/"
cp "$ICON" "$APP/Contents/Resources/agentplaza.icns"
sed "s/@VERSION@/$VERSION/g" "$HERE/Info.plist.in" > "$APP/Contents/Info.plist"

# Sign inside out: the VMM gets the hypervisor entitlement, the rest get none.
# The bundle is signed last and without --deep, which would strip the VMM entitlement.
if [ -f "$MACOS/apz-vmm" ]; then
  sign --entitlements "$HERE/entitlements.plist" "$MACOS/apz-vmm"
else
  warn "apz-vmm is not in the stages; the VMM will not have the hypervisor entitlement"
fi
# The helpers next; the main executable (agentplaza-desktop, CFBundleExecutable) is signed as the
# bundle itself, last, once everything it contains is signed.
for bin in "$MACOS"/*; do
  case "$(basename "$bin")" in
    apz-vmm|agentplaza-desktop) ;;
    *) sign --entitlements "$HERE/entitlements-app.plist" "$bin" ;;
  esac
done
sign --entitlements "$HERE/entitlements-app.plist" "$APP"
codesign --verify --strict --verbose=2 "$APP"

# DMG with the drag-to-Applications layout.
ln -s /Applications "$DMG_SRC/Applications"
mkdir -p "$OUT_DIR"
DMG="$OUT_DIR/AgentPlaza-$VERSION-macos-universal.dmg"
rm -f "$DMG"
hdiutil create -volname "AgentPlaza $VERSION" -srcfolder "$DMG_SRC" -ov -format UDZO "$DMG"

if [ "$SIGN_ID" != "-" ]; then
  codesign --force "${TS_FLAG[@]}" -s "$SIGN_ID" "$DMG"
fi
if [ -n "${APPLE_NOTARY_PROFILE:-}" ]; then
  xcrun notarytool submit "$DMG" --keychain-profile "$APPLE_NOTARY_PROFILE" --wait
  xcrun stapler staple "$DMG"
fi

echo "$DMG"
codesign -dv "$APP" 2>&1
