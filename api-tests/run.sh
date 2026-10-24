#!/usr/bin/env bash
# api-tests/run.sh APID OUT: the hub API's tests, against a scratch hub started from the apid binary
# APID, with every result and the HTML report in OUT. Run from the repository root; the pull request
# gate runs it in the CI image (pr.yml, job `api`).
#
#   1. Bruno (api-tests/bruno): the collection's requests and assertions. A failure fails the run.
#   2. Timings (bench.mjs): every parameterless GET in the spec, 20 times: p50, p95, size.
#   3. Schemathesis: requests generated from docs/api/openapi.json, looking for 5xx answers and
#      dropped connections. A server error fails the run.
#   4. The report (report/build.mjs): OUT/site/index.html.
#
# The hub is a scratch one: an empty env file (so no database and no keys), state in a temporary
# directory, no node daemon or hub behind it (ports 1 and 2 refuse), and requests from 127.0.0.1,
# which the hub takes as the owner's. Its HOME is in that directory too, so it never reads or writes the
# owner's own config. Bruno gets the hub's internal token as `internalToken`, for the agents' internal API.
# Tools: BRU (default `bru`), ST (default `st`), node.
# APZ_API_ONLY="folder ..." runs only those Bruno folders (each on its own: OUT/bruno-<folder>.json) and skips
# the timings, fuzzing and report, a quick check of one area while writing its tests; APZ_API_PORT picks the
# hub's port (default 17777).
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
mkdir -p "$state/home"
HOME="$state/home" XDG_CONFIG_HOME="$state/home/.config" XDG_DATA_HOME="$state/home/.local/share" \
"$apid" --port "$port" --state-dir "$state/hub" --env-file "$state/empty.env" \
  --vault-key "$state/vault.key" --noded-port 1 --hub-port 2 --console "$state/no-console" \
  > "$out/apid.log" 2>&1 &
hub=$!
# The hub's children (weft serve) are listed before the kill: once the hub is gone they are reparented to init.
trap 'kids="$(pgrep -P "$hub" || true)"; kill "$hub" $kids 2>/dev/null || true; wait "$hub" 2>/dev/null || true; rm -rf "$state"' EXIT

for _ in $(seq 60); do
  kill -0 "$hub" 2>/dev/null || { echo "the scratch hub exited:"; cat "$out/apid.log"; exit 1; }
  # Ready when this hub says it listens on the port (another server already on it would answer the curl too).
  grep -q "listening on http://127.0.0.1:$port" "$out/apid.log" && curl -sf "$base/v1/hub/identity" > /dev/null && break
  sleep 0.5
done
kill -0 "$hub" 2>/dev/null && grep -q "listening on http://127.0.0.1:$port" "$out/apid.log" ||
  { echo "the scratch hub is not listening on $port:"; cat "$out/apid.log"; exit 1; }

status=0
token="$(cat "$state/hub/internal.token")"
# The internal token stays out of the saved results.
bruno() {
  (cd api-tests/bruno && "$bru" run "$@" -r --env ci --env-var "baseUrl=$base" \
    --env-var "internalToken=$token" --env-var "budgetMs=${APZ_API_BUDGET_MS:-1000}" \
    --reporter-skip-headers x-apz-internal)
}
# Bruno skips a request file it cannot parse and still passes the run: the collection has no skipped requests, so
# any skip is a broken file, and fails the run with its parse error.
skipped() {
  node -e '
    const runs = [].concat(require(process.argv[1]));
    const bad = runs.flatMap((r) => r.results || []).filter((x) => x.status === "skipped");
    for (const x of bad) console.log(`skipped: ${x.test && x.test.filename}\n${x.error}`);
    process.exit(bad.length ? 1 : 0);' "$1"
}
echo "== bruno"
if [ -n "${APZ_API_ONLY:-}" ]; then
  # One run per folder: `bru run -r` with several folders runs the whole collection.
  : > "$out/bruno.log"
  for folder in $APZ_API_ONLY; do
    bruno "$folder" --reporter-json "$out/bruno-$folder.json" >> "$out/bruno.log" 2>&1 || status=1
    skipped "$out/bruno-$folder.json" || status=1
  done
  grep -E "Status|Requests|Tests  " "$out/bruno.log"
  exit "$status"
fi
bruno --reporter-json "$out/bruno.json" --reporter-junit "$out/bruno.xml" > "$out/bruno.log" 2>&1 || status=1
tail -n 25 "$out/bruno.log"
skipped "$out/bruno.json" || status=1

echo "== timings"
node api-tests/bench.mjs docs/api/openapi.json "$base" "$out/bench.json"

if [ "${APZ_API_FUZZ:-1}" = 1 ]; then
  echo "== schemathesis"
  node api-tests/fuzz-spec.mjs docs/api/openapi.json "$out/fuzz-spec.json"
  # The node-daemon routes are out of reach here (the scratch hub has no daemon), so they are excluded, as deepfuzz.sh does.
  st_status=0
  "$st" run "$out/fuzz-spec.json" --url "$base" --checks not_a_server_error --max-examples 3 \
    --exclude-path-regex '^/v1/(runs|volumes|templates|machine$|hub$|nodes/[^/]+/(runs|machine|stats))' \
    --request-timeout 5 --workers 2 --seed 1 --report junit --report-dir "$out/schemathesis" \
    > "$out/schemathesis.log" 2>&1 || st_status=$?
  tail -n 20 "$out/schemathesis.log"
  [ "$st_status" -eq 0 ] || { echo "schemathesis: exit $st_status (a server error, or the run broke)"; status=1; }
fi

echo "== report"
node api-tests/report/build.mjs "$out" "$out/site"
exit "$status"
