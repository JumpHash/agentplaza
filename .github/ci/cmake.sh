#!/usr/bin/env bash
# cmake.sh: puts cmake on PATH for the macOS and Windows jobs (the Linux image has its own). tree-sitter's
# wasm runtime (wasmtime's C API, under weft's `packs`) builds with it. The runner's own cmake when there is
# one; else Kitware's release, downloaded once into the runner's cache and checked against its SHA-256.
set -euo pipefail

if command -v cmake >/dev/null; then
  cmake --version | head -1
  exit 0
fi

VERSION=3.31.6
case "$(uname -s)" in
  Darwin)
    asset="cmake-$VERSION-macos-universal.tar.gz"
    sha=330b9514f5112e5ed4fb08b8b05803b776fd9b539a6ae12927d14dcc0ee2ba8d
    bin_rel="CMake.app/Contents/bin"
    ;;
  MINGW* | MSYS* | CYGWIN*)
    asset="cmake-$VERSION-windows-x86_64.zip"
    sha=d163cd3ab4959b0a53fa8988f2ddbd2e6c501658201e6a154386bad9dbe4f836
    bin_rel="bin"
    ;;
  *)
    echo "cmake.sh: no cmake release for $(uname -s); install cmake on this runner" >&2
    exit 1
    ;;
esac

dir="${CI_TOOLS_CACHE:-$HOME/.cache/agentplaza-ci}/cmake-$VERSION"
top="${asset%.tar.gz}"
top="${top%.zip}"
if [ ! -d "$dir/$top/$bin_rel" ]; then
  tmp=$(mktemp -d)
  curl -fsSL --retry 5 "https://github.com/Kitware/CMake/releases/download/v$VERSION/$asset" -o "$tmp/$asset"
  got=$( (sha256sum "$tmp/$asset" 2>/dev/null || shasum -a 256 "$tmp/$asset") | cut -d' ' -f1)
  if [ "$got" != "$sha" ]; then
    echo "cmake.sh: $asset has sha256 $got, expected $sha" >&2
    rm -rf "$tmp"
    exit 1
  fi
  mkdir -p "$dir"
  case "$asset" in
    *.tar.gz) tar -xzf "$tmp/$asset" -C "$dir" ;;
    *.zip)
      if command -v unzip >/dev/null; then
        unzip -q "$tmp/$asset" -d "$dir"
      else
        powershell -NoProfile -Command "Expand-Archive -LiteralPath '$(cygpath -w "$tmp/$asset")' -DestinationPath '$(cygpath -w "$dir")'"
      fi
      ;;
  esac
  rm -rf "$tmp"
fi

bin="$dir/$top/$bin_rel"
# GITHUB_PATH takes a native path on Windows.
if command -v cygpath >/dev/null; then
  cygpath -w "$bin" >> "$GITHUB_PATH"
else
  echo "$bin" >> "$GITHUB_PATH"
fi
"$bin/cmake" --version | head -1
