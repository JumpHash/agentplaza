#!/bin/sh
# in-image.sh <command>: runs a shell command in the CI image ($CI_IMAGE, built from this directory's
# Dockerfile) on a self-hosted Linux runner, as the runner's user, with the workspace mounted at the
# same path (so a later host step, an install check or the Actions cache, sees the same files).
#
# Cargo's registry and git checkouts persist on the runner under $APZ_CI_CACHE
# (default ~/.cache/agentplaza-ci), and so does the build directory (CARGO_TARGET_DIR=/ci-target)
# unless APZ_CI_TARGET=workspace: an unchanged crate is never compiled twice, and a release
# rebuilds only what changed.
#
# Passed through when set: VERSION, APZ_TEST_DATABASE_URL, APZ_ALLOW_NO_GUEST, CROSS_COMPILE,
# APZ_BUSYBOX, GH_TOKEN, GITHUB_REPOSITORY, GITHUB_SHA. APZ_CI_NETWORK joins a docker network
# (the tests' Postgres).
set -eu

: "${CI_IMAGE:?CI_IMAGE names the CI image}"
cache="${APZ_CI_CACHE:-$HOME/.cache/agentplaza-ci}"
mkdir -p "$cache/registry" "$cache/git" "$cache/target"

target_args=""
if [ "${APZ_CI_TARGET:-shared}" != workspace ]; then
  target_args="-v $cache/target:/ci-target -e CARGO_TARGET_DIR=/ci-target"
fi

# shellcheck disable=SC2086 # target_args is a list of words
exec docker run --rm \
  ${APZ_CI_NETWORK:+--network "$APZ_CI_NETWORK"} \
  -u "$(id -u):$(id -g)" -e HOME=/tmp --ulimit nofile=65536:65536 \
  -v "$PWD:$PWD" -w "$PWD" \
  -v "$cache/registry:/usr/local/cargo/registry" -v "$cache/git:/usr/local/cargo/git" \
  $target_args \
  -e CARGO_TERM_COLOR=always -e CARGO_INCREMENTAL=0 -e CARGO_NET_RETRY=10 \
  -e VERSION -e APZ_TEST_DATABASE_URL -e APZ_ALLOW_NO_GUEST -e CROSS_COMPILE -e APZ_BUSYBOX \
  -e GH_TOKEN -e GITHUB_REPOSITORY -e GITHUB_SHA ${GITHUB_REPOSITORY:+-e GH_REPO=$GITHUB_REPOSITORY} \
  "$CI_IMAGE" bash -euo pipefail -c "$*"
