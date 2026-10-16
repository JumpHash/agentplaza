#!/usr/bin/env bash
# changes.sh <base> <head>: what the commits from <base> to <head> touch, for pr.yml, release.yml
# and nightly.yml to decide what runs. Writes key=value lines to $GITHUB_OUTPUT (stdout without it):
#
#   code      product code changed: anything but the files below. Without it nothing builds or
#             tests, and main cuts no version and no release.
#   ci        the build itself changed (.github/workflows, .github/ci): the gate runs to test it
#   build     code or ci: the pull request gate builds, lints and tests
#   release   main should release: code changed, or a workflow the release runs (not pr.yml/nightly.yml)
#   web       the web console or what it compiles from (apps/web, the wasm crates, the engine)
#   platform  code that differs per OS: the platform crates, `#[cfg]`-gated Rust, packaging, the
#             lockfile and toolchain. Windows, macOS and Linux arm64 test only then.
#   trusted   $TRUSTED as given (pr.yml: false for a pull request from a fork), default true
#
# Not code: Markdown, docs/, .gitignore/.gitattributes/.editorconfig anywhere, LICENSE, VERSION,
# .github/CODEOWNERS, .github/actionlint.yaml, .claude/. An empty or unknown base counts as
# "everything changed".
set -euo pipefail

base="${1:-}" head="${2:-HEAD}"
out="${GITHUB_OUTPUT:-/dev/stdout}"

if [ -z "$base" ] || [ "$base" = 0000000000000000000000000000000000000000 ] || ! git cat-file -e "$base^{commit}" 2>/dev/null; then
  echo "no base to compare with: treating everything as changed" >&2
  printf '%s\n' code=true ci=true build=true release=true web=true platform=true "trusted=${TRUSTED:-true}" >> "$out"
  exit 0
fi

mapfile -t files < <(git diff --name-only "$(git merge-base "$base" "$head")" "$head")
printf 'changed: %s\n' "${#files[@]}" >&2

code=false ci=false release=false web=false platform=false
for f in "${files[@]}"; do
  case "$f" in
    *.md | docs/* | .claude/* | LICENSE* | VERSION | .github/CODEOWNERS | .github/actionlint.yaml) continue ;;
    .gitignore | */.gitignore | .gitattributes | */.gitattributes | .editorconfig | */.editorconfig) continue ;;
    .github/workflows/pr.yml | .github/workflows/nightly.yml | .github/ci/changes.sh) ci=true; continue ;;
    .github/workflows/* | .github/ci/*) ci=true release=true ;;
    *) code=true release=true ;;
  esac
  case "$f" in
    apps/web/* | crates/apz-engine/* | crates/apz-engine-web/* | crates/apz-ide/* | crates/apz-ide-web/* | \
    crates/apz-metrics-view/* | crates/apz-board/* | .github/workflows/console.yml) web=true ;;
  esac
  case "$f" in
    crates/apz-desktop*/* | crates/vmm/* | crates/noded/* | crates/vsockd/* | crates/vmctl/* | \
    crates/apz-uds/* | crates/apz-os/* | crates/acpi-tables/* | apps/desktop/* | packaging/* | \
    Cargo.lock | Cargo.toml | rust-toolchain* | .cargo/* | .config/nextest.toml | \
    .github/workflows/linux.yml | .github/workflows/windows.yml | .github/workflows/macos.yml | .github/ci/*)
      platform=true ;;
    *.rs)
      # Rust that compiles differently per OS.
      if git cat-file -e "$head:$f" 2>/dev/null && git show "$head:$f" |
        grep -q -E 'cfg\(.*(windows|unix|target_os|target_family|target_vendor|target_arch)'; then
        platform=true
      fi ;;
  esac
done
# The gate checks itself when CI changes.
build=false
if [ "$ci" = true ]; then web=true build=true; fi
if [ "$code" = true ]; then build=true; fi

printf '%s\n' "code=$code" "ci=$ci" "build=$build" "release=$release" "web=$web" "platform=$platform" "trusted=${TRUSTED:-true}" | tee -a "$out" >&2
