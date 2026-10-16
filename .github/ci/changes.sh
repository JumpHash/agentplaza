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
#             lockfile and toolchain. Windows and macOS test only then.
#   tests     the workspace packages whose tests a pull request runs: the changed ones (their tests
#             and those of every package that depends on them run), or `all` when a change reaches
#             past a package's own directory (the lockfile, a root manifest, CI, sql/, assets/,
#             apps/web: files more than one crate reads). The merge queue tests everything.
#   trusted   $TRUSTED as given (pr.yml: false for a pull request from a fork), default true
#
# Not code: Markdown, docs/ (but for what the code compiles in or its tests read: docs/agents and
# docs/api), .gitignore/.gitattributes/.editorconfig anywhere, LICENSE, VERSION,
# .github/CODEOWNERS, .github/actionlint.yaml, .claude/. An empty or unknown base counts as
# "everything changed".
set -euo pipefail

base="${1:-}" head="${2:-HEAD}"
out="${GITHUB_OUTPUT:-/dev/stdout}"

if [ -z "$base" ] || [ "$base" = 0000000000000000000000000000000000000000 ] || ! git cat-file -e "$base^{commit}" 2>/dev/null; then
  echo "no base to compare with: treating everything as changed" >&2
  printf '%s\n' code=true ci=true build=true release=true web=true platform=true tests=all "trusted=${TRUSTED:-true}" >> "$out"
  exit 0
fi

mapfile -t files < <(git diff --name-only "$(git merge-base "$base" "$head")" "$head")
printf 'changed: %s\n' "${#files[@]}" >&2

# The workspace's members (Cargo.toml's `members`, globs one level deep like crates/*).
mapfile -t members < <(git show "$head:Cargo.toml" | sed -n 's/^members *= *\[\(.*\)\]/\1/p' | tr -d '" ' | tr ',' '\n')
# package <file>: the name of the member package whose directory holds <file>, or nothing.
package() {
  local dir m
  dir=$(dirname "$1")
  while [ "$dir" != . ] && ! git cat-file -e "$head:$dir/Cargo.toml" 2>/dev/null; do dir=$(dirname "$dir"); done
  [ "$dir" != . ] || return 0
  for m in "${members[@]}"; do
    if [ "$dir" = "$m" ] || { [ "${m%/\*}" != "$m" ] && [ "$(dirname "$dir")" = "${m%/*}" ]; }; then
      git show "$head:$dir/Cargo.toml" | sed -n '/^\[package\]/,/^\[/s/^name *= *"\(.*\)"/\1/p' | head -n 1
      return 0
    fi
  done
}

code=false ci=false release=false web=false platform=false tests=""
for f in "${files[@]}"; do
  case "$f" in
    docs/agents/* | docs/api/*) code=true release=true ;;
    *.md | docs/* | .claude/* | LICENSE* | VERSION | .github/CODEOWNERS | .github/actionlint.yaml) continue ;;
    .gitignore | */.gitignore | .gitattributes | */.gitattributes | .editorconfig | */.editorconfig) continue ;;
    .github/workflows/pr.yml | .github/workflows/nightly.yml | .github/ci/changes.sh) ci=true; continue ;;
    .github/workflows/* | .github/ci/*) ci=true release=true ;;
    *) code=true release=true ;;
  esac
  # Which tests: the package the file is in; `all` past one package's directory (or for CI).
  if [ "$tests" != all ]; then
    name=$(package "$f")
    # CI changes test everything; a name that is not a plain package name too (it reaches a command line).
    case "$f" in .github/*) name="" ;; esac
    case "$name" in *[!A-Za-z0-9_-]*) name="" ;; esac
    if [ -z "$name" ]; then tests=all
    elif [[ " $tests " != *" $name "* ]]; then tests="${tests:+$tests }$name"; fi
  fi
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
[ -n "$tests" ] || tests=all

printf '%s\n' "code=$code" "ci=$ci" "build=$build" "release=$release" "web=$web" "platform=$platform" "tests=$tests" "trusted=${TRUSTED:-true}" | tee -a "$out" >&2
