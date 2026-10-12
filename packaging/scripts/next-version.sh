#!/usr/bin/env bash
# Prints the next release version (no "v") and the previous tag, from the commits since the last
# `v<semver>` tag (packaging/README.md, "Versions"):
#   major  a commit body has "BREAKING CHANGE:", or a subject reads "<scope>!: …"
#          (while the major version is 0, a breaking change bumps the minor version)
#   minor  a commit carries the trailer "Release: minor"
#   patch  everything else
#   A "Release: major|minor|patch" trailer forces at least that bump; $FORCE_BUMP (a manual run's
#   input) overrides everything. With no tag yet, the workspace version (Cargo.toml) is released.
# Output (also appended to $GITHUB_OUTPUT when set): version=<x.y.z> previous=<tag or empty>
set -euo pipefail
cd "$(dirname "$0")/../.."

workspace=$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p}' Cargo.toml | head -1)
previous=$(git describe --tags --abbrev=0 --match 'v[0-9]*.[0-9]*.[0-9]*' 2>/dev/null || true)

if [ -z "$previous" ]; then
  version=$workspace
else
  IFS=. read -r major minor patch <<< "${previous#v}"
  patch=${patch%%[-+]*}
  log=$(git log --format='%s%n%b%n--end--' "$previous..HEAD")
  bump=patch
  if grep -qiE '^Release: *minor' <<< "$log"; then bump=minor; fi
  if grep -qiE '^Release: *major' <<< "$log" \
     || grep -qE '^BREAKING[ -]CHANGE:' <<< "$log" \
     || grep -qE '^[A-Za-z0-9_./-]+(\([^)]*\))?!:' <<< "$log"; then bump=major; fi
  bump=${FORCE_BUMP:-$bump}
  if [ "$bump" = major ] && [ "$major" = 0 ]; then bump=minor; fi
  case $bump in
    major) version="$((major + 1)).0.0" ;;
    minor) version="$major.$((minor + 1)).0" ;;
    patch) version="$major.$minor.$((patch + 1))" ;;
    *) echo "next-version: unknown bump '$bump'" >&2; exit 1 ;;
  esac
fi

echo "version=$version"
echo "previous=$previous"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  { echo "version=$version"; echo "previous=$previous"; } >> "$GITHUB_OUTPUT"
fi
