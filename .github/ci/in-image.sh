#!/bin/sh
# in-image.sh <command>: runs a shell command in the CI image ($CI_IMAGE, built from this directory's
# Dockerfile) on a self-hosted Linux runner, as the runner's user, with the workspace mounted at the
# same path (so a later host step, an install check or the Actions cache, sees the same files).
#
# Cargo's registry and git checkouts persist on the runner under $APZ_CI_CACHE
# (default ~/.cache/agentplaza-ci), and so does the build directory (CARGO_TARGET_DIR=/ci-target)
# unless APZ_CI_TARGET=workspace: an unchanged crate is never compiled twice, and a release
# rebuilds only what changed (with the checkout's mtimes kept by build-cache.py mtimes). npm's
# download cache persists there too. No sccache: the runner fleet's storage guard deletes ~/.cache
# when the host's disk is short, so a second copy of the compiled crates there would only add to
# that pressure and be deleted with the build directory.
#
# Passed through when set: VERSION, APZ_ALLOW_NO_GUEST, APZ_REQUIRE_ENGINE, CROSS_COMPILE,
# APZ_BUSYBOX, APZ_PERF_SLACK, CI (the tests' timing budgets read it), GH_TOKEN, GITHUB_REPOSITORY, GITHUB_SHA.
# APZ_CI_NETWORK joins a docker network.
set -eu

: "${CI_IMAGE:?CI_IMAGE names the CI image}"
cache="${APZ_CI_CACHE:-$HOME/.cache/agentplaza-ci}"
mkdir -p "$cache/registry" "$cache/git" "$cache/target" "$cache/npm"

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

# Cargo's parallel compiles: one per 4 GiB the host has available, at most one per core. Four release builds of the big
# crates at once (the hub, the desktop, the engine and its web build) went past an 11 GB runner's memory, and the kernel
# killed the job (exit 137) with nothing in its log. A runner that sets CARGO_BUILD_JOBS keeps its own.
jobs="${CARGO_BUILD_JOBS:-}"
if [ -z "$jobs" ] && [ -r /proc/meminfo ]; then
  avail_kib=$(awk '/^MemAvailable:/ {print $2}' /proc/meminfo)
  jobs=$((avail_kib / (4 * 1024 * 1024)))
  cores=$(nproc)
  [ "$jobs" -gt "$cores" ] && jobs=$cores
  [ "$jobs" -lt 1 ] && jobs=1
  echo "in-image: $jobs parallel compiles ($((avail_kib / 1024 / 1024)) GiB available, $cores cores)" >&2
fi

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
  -v "$cache/npm:/ci-npm" -e npm_config_cache=/ci-npm -e npm_config_prefer_offline=true \
  -e CARGO_TERM_COLOR=always -e CARGO_NET_RETRY=10 ${jobs:+-e CARGO_BUILD_JOBS=$jobs} \
  -e VERSION -e APZ_ALLOW_NO_GUEST -e APZ_REQUIRE_ENGINE -e CROSS_COMPILE -e APZ_BUSYBOX \
  -e APZ_PERF_SLACK -e CI \
  -e GH_TOKEN -e GITHUB_REPOSITORY -e GITHUB_SHA ${GITHUB_REPOSITORY:+-e GH_REPO=$GITHUB_REPOSITORY} \
  "$CI_IMAGE" bash -euo pipefail -c "$*"
