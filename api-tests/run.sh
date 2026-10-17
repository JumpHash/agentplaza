#!/usr/bin/env bash
# api-tests/run.sh APID OUT: the hub API's tests, against a scratch hub started from the apid binary
# APID, with every result and the HTML report in OUT. Run from the repository root; the pull request
# gate runs it in the CI image (pr.yml, job `api`).
#
#   1. Bruno (api-tests/bruno): the collection's requests and assertions. A failure fails the run.
#   2. Timings (bench.mjs): every parameterless GET in the spec, 20 times: p50, p95, size.
#   3. Schemathesis: requests generated from docs/api/openapi.json, looking for 5xx answers and
#      dropped connections. Reported, not (yet) failing the run.
#   4. The report (report/build.mjs): OUT/site/index.html.
#
# The hub is a scratch one: an empty env file (so no database and no keys), state in a temporary
# directory, no node daemon or hub behind it (ports 1 and 2 refuse), and requests from 127.0.0.1,
# which the hub takes as the owner's. Tools: BRU (default `bru`), ST (default `st`), node.
# APZ_API_FUZZ=0 skips Schemathesis (a quicker local run).
set -euo pipefail

apid="$(realpath "$1")"
mkdir -p "$2"
out="$(realpath "$2")"
port="${APZ_API_PORT:-17777}"
base="http://127.0.0.1:$port"
bru="${BRU:-bru}"
st="${ST:-st}"

state="$(mktemp -d)"
: > "$state/empty.env"
"$apid" --port "$port" --state-dir "$state/hub" --env-file "$state/empty.env" \
  --vault-key "$state/vault.key" --noded-port 1 --hub-port 2 --console "$state/no-console" \
  > "$out/apid.log" 2>&1 &
hub=$!
trap 'kill "$hub" 2>/dev/null || true; wait "$hub" 2>/dev/null || true; rm -rf "$state"' EXIT

for _ in $(seq 60); do
  curl -sf "$base/v1/hub/identity" > /dev/null && break
  kill -0 "$hub" 2>/dev/null || { echo "the scratch hub exited:"; cat "$out/apid.log"; exit 1; }
  sleep 0.5
done
grep -q "no APZ_DATABASE_URL" "$out/apid.log" || { echo "the scratch hub found a database: refusing"; exit 1; }

status=0
echo "== bruno"
(cd api-tests/bruno && "$bru" run -r --env ci --env-var "baseUrl=$base" \
  --reporter-json "$out/bruno.json" --reporter-junit "$out/bruno.xml") > "$out/bruno.log" 2>&1 || status=1
tail -n 25 "$out/bruno.log"

echo "== timings"
node api-tests/bench.mjs docs/api/openapi.json "$base" "$out/bench.json"

if [ "${APZ_API_FUZZ:-1}" = 1 ]; then
  echo "== schemathesis (reported, not failing)"
  node api-tests/fuzz-spec.mjs docs/api/openapi.json "$out/fuzz-spec.json"
  "$st" run "$out/fuzz-spec.json" --url "$base" --checks not_a_server_error --max-examples 3 \
    --request-timeout 5 --workers 2 --seed 1 --report junit --report-dir "$out/schemathesis" \
    > "$out/schemathesis.log" 2>&1 || true
  tail -n 20 "$out/schemathesis.log"
fi

echo "== report"
node api-tests/report/build.mjs "$out" "$out/site"
exit "$status"
