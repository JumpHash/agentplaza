#!/usr/bin/env bash
# Builds the AgentPlaza guest kernel for one architecture: a pinned long-term kernel from
# kernel.org (checksum-verified), `make tinyconfig` plus scripts/guest/{common,<arch>}.config,
# every device built in (no modules). The VMM boots it with the initrd from scripts/mk-initrd.sh.
#
#   scripts/mk-guest-kernel.sh <x86_64|aarch64> [out-file]
#
# Output: target/guest/<arch>/vmlinux (x86_64: a bzImage; aarch64: the arm64 `Image`), and
# target/guest/<arch>/kernel.config (the final configuration). Builds natively, or cross-builds
# with CROSS_COMPILE (e.g. CROSS_COMPILE=aarch64-linux-gnu-). Needs make, gcc, flex, bison, bc,
# libelf, openssl headers, xz and curl.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=6.18.56
SHA256=4633bbda2ab95eaf2a7af4041236451d6759c72e6946265531af4b3aacefbf89
URL="https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-$VERSION.tar.xz"

arch=${1:?usage: mk-guest-kernel.sh <x86_64|aarch64> [out-file]}
case "$arch" in
  x86_64) karch=x86_64; image=arch/x86/boot/bzImage; target=bzImage ;;
  aarch64) karch=arm64; image=arch/arm64/boot/Image; target=Image ;;
  *) echo "mk-guest-kernel: unknown architecture $arch (x86_64 or aarch64)" >&2; exit 1 ;;
esac
out_dir="target/guest/$arch"
out=${2:-$out_dir/vmlinux}
cache="target/guest/src"
src="$cache/linux-$VERSION"
build="$cache/build-$arch"
mkdir -p "$cache" "$out_dir" "$(dirname "$out")"

# The source, once, verified against the pinned checksum.
tarball="$cache/linux-$VERSION.tar.xz"
if [ ! -f "$tarball" ]; then
  echo "mk-guest-kernel: fetching linux-$VERSION"
  curl -fsSL --retry 3 --max-time 600 -o "$tarball.part" "$URL"
  mv "$tarball.part" "$tarball"
fi
echo "$SHA256  $tarball" | sha256sum -c --quiet - || {
  echo "mk-guest-kernel: $tarball does not match the pinned checksum" >&2
  rm -f "$tarball"
  exit 1
}
if [ ! -d "$src" ]; then
  tar -xf "$tarball" -C "$cache"
fi

# The configuration: tinyconfig, then our fragments, then defaults for whatever they imply.
rm -rf "$build"
mkdir -p "$build"
kmake() { make -s -C "$src" O="$(pwd)/$build" ARCH="$karch" ${CROSS_COMPILE:+CROSS_COMPILE=$CROSS_COMPILE} "$@"; }
kmake tinyconfig
"$src/scripts/kconfig/merge_config.sh" -m -O "$build" "$build/.config" scripts/guest/common.config "scripts/guest/$arch.config" >/dev/null
kmake olddefconfig

# Every `=y` we asked for must have survived (a renamed or unsatisfiable option fails loudly).
missing=0
for fragment in scripts/guest/common.config "scripts/guest/$arch.config"; do
  while IFS= read -r line; do
    case "$line" in CONFIG_*=y) ;; *) continue ;; esac
    if ! grep -qx "$line" "$build/.config"; then
      echo "mk-guest-kernel: $line (from $fragment) is not in the final configuration" >&2
      missing=1
    fi
  done < "$fragment"
done
[ "$missing" = 0 ] || exit 1

echo "mk-guest-kernel: building linux-$VERSION for $arch"
kmake -j"$(nproc)" "$target"
cp "$build/$image" "$out"
cp "$build/.config" "$out_dir/kernel.config"
echo "$VERSION" > "$out_dir/kernel.version"
ls -la "$out"
