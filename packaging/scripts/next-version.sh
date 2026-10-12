#!/usr/bin/env bash
# Prints the next release version (no "v"), the previous release tag, and where the version came
# from (packaging/README.md, "Versions"). The repository's VERSION file holds the current release;
# releases are tagged v<major>.<minor>.<patch>.
#
#   manual  $OVERRIDE (a manual run's `version` input), if set
#   file    VERSION holds a version above the last release: a merged change raised it (the release
#           job writes each released version back to VERSION, so otherwise it equals the last one)
#   auto    otherwise: the last release plus one, each place rolling over past 9 into the next:
#           0.0.1 → 0.0.2 … 0.0.9 → 0.1.0 … 0.9.9 → 1.0.0 … 9.9.9 → 10.0.0 (major never rolls)
#
# With no release yet the last release counts as 0.0.0, so the first automatic one is 0.0.1.
# Output (also appended to $GITHUB_OUTPUT when set): version=… previous=… source=…
set -euo pipefail
cd "$(dirname "$0")/../.."

semver='^[0-9]+\.[0-9]+\.[0-9]+$'
die() { echo "next-version: $*" >&2; exit 1; }

# "a > b" for x.y.z versions.
greater() {
  local a b
  IFS=. read -ra a <<< "$1"
  IFS=. read -ra b <<< "$2"
  for i in 0 1 2; do
    if (( a[i] != b[i] )); then (( a[i] > b[i] )); return; fi
  done
  return 1
}

previous=$(git tag --list 'v[0-9]*.[0-9]*.[0-9]*' --sort=-v:refname | head -1)
last=${previous#v}
last=${last:-0.0.0}

file=""
if [ -f VERSION ]; then
  file=$(tr -d '[:space:]' < VERSION)
fi
if [ -n "${OVERRIDE:-}" ]; then
  version=${OVERRIDE#v}
  [[ "$version" =~ $semver ]] || die "the version '$OVERRIDE' is not <major>.<minor>.<patch>"
  greater "$version" "$last" || die "the version $version is not above the last release $last"
  source=manual
elif [[ "$file" =~ $semver ]] && greater "$file" "$last"; then
  version=$file
  source=file
else
  if [ -n "$file" ] && ! [[ "$file" =~ $semver ]]; then
    echo "next-version: VERSION holds '$file', not <major>.<minor>.<patch>; releasing the next one" >&2
  fi
  IFS=. read -r major minor patch <<< "$last"
  patch=$((patch + 1))
  if (( patch > 9 )); then patch=0; minor=$((minor + 1)); fi
  if (( minor > 9 )); then minor=0; major=$((major + 1)); fi
  version="$major.$minor.$patch"
  source=auto
fi

echo "version=$version"
echo "previous=$previous"
echo "source=$source"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  { echo "version=$version"; echo "previous=$previous"; echo "source=$source"; } >> "$GITHUB_OUTPUT"
fi
