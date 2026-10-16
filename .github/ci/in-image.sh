#!/bin/sh
# in-image.sh <command>: runs a shell command in the CI image ($CI_IMAGE, built from this directory's
# Dockerfile) on a self-hosted Linux runner, as the runner's user, with the workspace mounted at the
# same path (so a later host step, an install check or the Actions cache, sees the same files).
#
# Cargo's registry and git checkouts persist on the runner under $APZ_CI_CACHE
# (default ~/.cache/agentplaza-ci), and so does the build directory (CARGO_TARGET_DIR=/ci-target)
# unless APZ_CI_TARGET=workspace: an unchanged crate is never compiled twice, and a release
# rebuilds only what changed (with the checkout's mtimes kept by build-cache.py mtimes). Behind it,
# sccache (20 GB, also on the runner) hands back any library crate compiled before with the same
# source and flags, for what the build directory does not have: units the prune removed, the
# guest's build in the workspace, a job's first build of a feature set. npm's download cache
# persists there too.
#
# Passed through when set: VERSION, APZ_TEST_DATABASE_URL, APZ_ALLOW_NO_GUEST, APZ_REQUIRE_ENGINE, CROSS_COMPILE,
# APZ_BUSYBOX, APZ_PERF_SLACK, CI (the tests' timing budgets read it), GH_TOKEN, GITHUB_REPOSITORY, GITHUB_SHA.
# APZ_CI_NETWORK joins a docker network
# (the tests' Postgres).
set -eu

: "${CI_IMAGE:?CI_IMAGE names the CI image}"
cache="${APZ_CI_CACHE:-$HOME/.cache/agentplaza-ci}"
mkdir -p "$cache/registry" "$cache/git" "$cache/target" "$cache/sccache" "$cache/npm"

# The build runs as the runner's user, so what it writes to the workspace and the caches is the
# runner's. Where Docker maps users (the arm64 runners), that takes a different flag: rootless Docker
# maps the container's root to the runner's user, so the build runs as root inside; a daemon with
# userns-remap maps every uid to a range of its own, so this container opts out (--userns=host) and
# runs as the runner's uid, no more than the runner itself may do.
user="$(id -u):$(id -g)" userns=""
case " $(docker info --format '{{join .SecurityOptions " "}}' 2>/dev/null) " in
  *" name=rootless"*) user="0:0" ;;
  *" name=userns"*) userns="--userns=host" ;;
esac

target_args=""
if [ "${APZ_CI_TARGET:-shared}" != workspace ]; then
  target_args="-v $cache/target:/ci-target -e CARGO_TARGET_DIR=/ci-target"
fi

# shellcheck disable=SC2086 # target_args is a list of words, userns one or none
exec docker run --rm \
  ${APZ_CI_NETWORK:+--network "$APZ_CI_NETWORK"} \
  $userns -u "$user" -e HOME=/tmp --ulimit nofile=65536:65536 \
  -v "$PWD:$PWD" -w "$PWD" \
  -v "$cache/registry:/usr/local/cargo/registry" -v "$cache/git:/usr/local/cargo/git" \
  $target_args \
  -v "$cache/sccache:/ci-sccache" -e RUSTC_WRAPPER=sccache -e SCCACHE_DIR=/ci-sccache -e SCCACHE_CACHE_SIZE=20G \
  -v "$cache/npm:/ci-npm" -e npm_config_cache=/ci-npm -e npm_config_prefer_offline=true \
  -e CARGO_TERM_COLOR=always -e CARGO_INCREMENTAL=0 -e CARGO_NET_RETRY=10 \
  -e VERSION -e APZ_TEST_DATABASE_URL -e APZ_ALLOW_NO_GUEST -e APZ_REQUIRE_ENGINE -e CROSS_COMPILE -e APZ_BUSYBOX \
  -e APZ_PERF_SLACK -e CI \
  -e GH_TOKEN -e GITHUB_REPOSITORY -e GITHUB_SHA ${GITHUB_REPOSITORY:+-e GH_REPO=$GITHUB_REPOSITORY} \
  "$CI_IMAGE" bash -c '
    bash -euo pipefail -c "$1"; status=$?
    # Stopping the server writes out what it still holds; its counts say what the cache saved.
    sccache --stop-server 2>/dev/null | grep -E "^Cache (hits|misses) +[0-9]" | tr -s " " || true
    exit $status' in-image "$*"
