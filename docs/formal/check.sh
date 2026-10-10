#!/usr/bin/env bash
# Model-check the specs with TLC, check that each mutant (a deliberately broken variant) is caught,
# and that each witness (a situation the invariants talk about) is actually reachable.
# Needs Java and tla2tools.jar: TLA2TOOLS=/path/to/tla2tools.jar ./check.sh
set -uo pipefail
cd "$(dirname "$0")"
JAR="${TLA2TOOLS:?set TLA2TOOLS to tla2tools.jar}"
meta="$(mktemp -d)"; trap 'rm -rf "$meta"' EXIT
fail=0
run() { (cd "$(dirname "$1")" && java -XX:+UseParallelGC -cp "$JAR" tlc2.TLC -workers auto -metadir "$meta/$(basename "$1")" \
          -config "$(basename "$1").cfg" "$(basename "$1").tla" 2>&1 | grep -v '^Picked up'); }
for spec in AgentSession Approval Economy TaskLifecycle RestoreMerge; do
  out="$(run "$spec")"
  if grep -q "No error has been found" <<<"$out"; then
    echo "ok    $spec: $(grep -o '[0-9,]* distinct states found' <<<"$out" | tail -1), no invariant violated"
  else echo "FAIL  $spec"; grep -E "Error|violated" <<<"$out" | head -5; fail=1; fi
done
python3 make_mutants.py >/dev/null || { echo "FAIL  mutant generation"; exit 1; }
for m in mutants/*.tla; do
  name="${m%.tla}"
  out="$(run "$name")"
  if [[ "$name" == *_Reach_* ]]; then      # witness: TLC must find a state where it happens
    if grep -q "Invariant NeverHappens is violated" <<<"$out"; then echo "ok    $(basename "$name") reachable"
    else echo "FAIL  $(basename "$name") is unreachable (invariants may pass vacuously)"; fail=1; fi
  elif grep -qE "is violated|Deadlock reached" <<<"$out"; then
    echo "ok    $(basename "$name") caught: $(grep -oE '(Invariant|property) [A-Za-z]* is violated|Deadlock reached' <<<"$out" | head -1)"
  else echo "FAIL  $(basename "$name") was NOT caught"; grep -E "Error|deadlock" <<<"$out" | head -3; fail=1; fi
done
exit $fail
