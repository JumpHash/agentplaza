#!/usr/bin/env bash
# coverage.sh OUT: measures the code coverage of each kind of test on this checkout, into OUT (data only:
# the reports .github/ci/coverage.py reads). Run from the repository root, in the CI image (pr.yml, job
# `coverage`).
#
#   Rust unit tests         the lib and bin targets' tests (nextest), instrumented by cargo-llvm-cov
#   Rust integration tests  the tests/ targets, the same way; and both runs together
#   Hub API (Bruno)         apid built instrumented, and api-tests/run.sh against it (its Bruno collection,
#                           no fuzzing): the code the requests reach, read with llvm-cov
#   Web console             apps/web's tests (test/run.mjs's bundle, kept) under V8 coverage, mapped to the
#                           sources by c8
#
# Every instrumented build goes to its own directory (CARGO_TARGET_DIR, /ci-target/coverage in the CI image),
# so the normal build cache is not touched; the workspace is rebuilt instrumented each time (cargo-llvm-cov's
# clean), its dependencies are not. APZ_COV_SCOPE="-p crate ..." runs a subset (a local try).
set -euo pipefail
shopt -s nullglob

out="$(realpath -m "$1")"
rm -rf "$out"
mkdir -p "$out"
raw="$out/raw"

export CARGO_TARGET_DIR="${APZ_COV_TARGET:-/ci-target/coverage}"
export CARGO_LLVM_COV_TARGET_DIR="$CARGO_TARGET_DIR"
# The timing budgets' multiplier (the tests read it): instrumented code runs several times slower.
export APZ_PERF_SLACK="${APZ_PERF_SLACK:-10}"
scope="${APZ_COV_SCOPE:---workspace}"
C8="${C8:-c8}"
BRU="${BRU:-bru}"
export BRU

# cargo-llvm-cov writes one profile file per test process into CARGO_LLVM_COV_TARGET_DIR. take DIR moves them
# into DIR, so each report reads only its own run; give DIR puts them back for a report.
take() {
  local files=("$CARGO_LLVM_COV_TARGET_DIR"/*.profraw)
  mkdir -p "$1"
  if ((${#files[@]})); then mv -f -- "${files[@]}" "$1"/; fi
}
give() {
  local files=("$1"/*.profraw)
  if ((${#files[@]})); then mv -f -- "${files[@]}" "$CARGO_LLVM_COV_TARGET_DIR"/; fi
}

echo "== Rust unit tests (instrumented)"
cargo llvm-cov clean --workspace
# shellcheck disable=SC2086 # $scope is a list of package options
cargo llvm-cov nextest --no-report --locked --release $scope -E 'kind(lib) | kind(bin)'
take "$raw/unit"

echo "== Rust integration tests (instrumented)"
# shellcheck disable=SC2086
cargo llvm-cov nextest --no-report --locked --release $scope -E 'kind(test)'
take "$raw/integration"

echo "== Rust reports"
give "$raw/unit"
cargo llvm-cov report --release --lcov --output-path "$out/rust-unit.lcov"
take "$raw/unit"
give "$raw/integration"
cargo llvm-cov report --release --lcov --output-path "$out/rust-integration.lcov"
take "$raw/integration"
give "$raw/unit"
give "$raw/integration"
cargo llvm-cov report --release --lcov --output-path "$out/rust-all.lcov"
cargo llvm-cov clean --profraw-only

echo "== Hub API (Bruno, against an instrumented apid)"
api_bin="$CARGO_TARGET_DIR/release/apid"
# The instrumentation and profile file cargo-llvm-cov gives its builds (the workspace crates only), for apid's build
# here and for the hub that api-tests/run.sh starts from it.
env_sh="$(cargo llvm-cov show-env --sh --release)"
eval "$env_sh"
# Weft too: the hub runs the `weft` beside it as a process, so it is this commit's and counts toward Weft's coverage.
cargo build --locked --release -p apid --bin apid -p weft --bin weft
[ -x "$api_bin" ] || { echo "no instrumented apid at $api_bin"; exit 1; }
# A profile is written when a process exits, and apid does not handle SIGTERM (run.sh stops the hub with one). The
# wrapper preloads profile-exit.so, which makes that signal an exit; exec keeps the hub's PID, the one run.sh stops.
mkdir -p "$raw"
shim="$(realpath -m "$raw/profile-exit.so")"
cc -shared -fPIC -O1 -o "$shim" .github/ci/profile-exit.c
hub_wrap="$raw/apid-wrapper"
printf '#!/bin/sh\nLD_PRELOAD="%s" exec "%s" "$@"\n' "$shim" "$api_bin" > "$hub_wrap"
chmod +x "$hub_wrap"
# A failing request fails the run's status, not the measurement: the hub ran the requests, and the pull request's
# api job is what gates them. The status is kept (api-status) and shown in the log.
api_status=0
# The instrumented hub is slower, and this run measures coverage, not latency: a plain request has 5 s, not 1.
APZ_API_BUDGET_MS=5000 api-tests/run.sh "$hub_wrap" "$out/api-run" || api_status=$?
echo "$api_status" > "$out/api-status"
if [ "$api_status" != 0 ]; then echo "::warning::the API run failed (status $api_status): its coverage is measured as it ran"; fi
take "$raw/api"
ls "$raw"/api/*.profraw >/dev/null 2>&1 || { echo "::error::the hub wrote no profile: its coverage is missing"; exit 1; }
llvm="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"
"$llvm/llvm-profdata" merge -sparse "$raw"/api/*.profraw -o "$raw/api.profdata"
"$llvm/llvm-cov" export -format=lcov -instr-profile="$raw/api.profdata" \
  -ignore-filename-regex='/\.cargo/registry/|/\.rustup/|/rustc/|/(tests|benches|examples)/' \
  "$api_bin" > "$out/hub-api.lcov"

echo "== Web console tests (V8 coverage)"
(
  cd apps/web
  # The symlink-free checkout installs its modules; a local tree that has them keeps them.
  [ -d node_modules ] || npm ci --no-audit --no-fund
  # test/run.mjs's bundle, kept on disk: c8 maps the coverage back to the TypeScript sources through its source map.
  rm -rf .coverage && mkdir .coverage
  ./node_modules/.bin/esbuild test/tests.ts --bundle --platform=node --format=esm \
    --loader:.css=empty --sourcemap --outfile=.coverage/tests.mjs --log-level=warning
  NODE_V8_COVERAGE=.coverage/v8 node .coverage/tests.mjs
  "$C8" report --temp-directory=.coverage/v8 --reporter=lcovonly --report-dir=.coverage/report >/dev/null
  cp .coverage/report/lcov.info "$out/web.lcov"
  rm -rf .coverage
)

echo "== The workspace's packages"
cargo metadata --no-deps --format-version 1 --locked > "$out/metadata.json"

rm -rf "$raw"
echo "coverage data: $out"
