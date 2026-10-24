#!/usr/bin/env bash
# The Mac path end to end, headless: from starting the plaza's services to a chat with the main
# orchestrator, through the stub provider (`--dev-models`: mock/agent, offline), in a scratch
# directory. The services are the ones the desktop app's "this machine" plan starts (apid, vsockd,
# noded), with the same arguments, on ports that are not the desktop's (7700, 8899, 19080) so a
# running hub of the owner is never touched. The orchestrator runs in its own VM through
# Hypervisor.framework; apz-vmm is signed with the hypervisor entitlement (scripts/hvf-run.sh).
#
#   scripts/e2e-mac.sh            run it; prints the timing; exits 0 when the answer came, 1 when not
#   APZ_E2E_KEEP=1 scripts/e2e-mac.sh   keep the scratch directory (logs, state) for reading
#
# Needs the binaries (cargo build -p apid -p noded -p vsockd -p vmm -p weft) and the guest
# (target/guest/aarch64/vmlinux and initrd.gz, from scripts/mk-guest.sh aarch64). Skips with a
# reason, exit 0, when the Mac is not Apple silicon or a file is missing.
set -euo pipefail
cd "$(dirname "$0")/.."

skip() { echo "e2e-mac: skipped: $*"; exit 0; }
[ "$(uname -s)" = Darwin ] || skip "not macOS"
[ "$(uname -m)" = arm64 ] || skip "not Apple silicon (arm64): Hypervisor.framework VMs need it"
GUEST=target/guest/aarch64
for f in vmlinux initrd.gz; do
  [ -f "$GUEST/$f" ] || skip "$GUEST/$f is missing (scripts/mk-guest.sh aarch64 builds it)"
done
BIN=${APZ_E2E_BIN:-target/debug}
for b in apid noded vsockd apz-vmm weft; do
  [ -x "$BIN/$b" ] || skip "$BIN/$b is not built (cargo build -p apid -p noded -p vsockd -p vmm -p weft)"
done

# Ports of this run: not the desktop's 7700, 8899 and 19080, nor the owner's hub's.
API=17911; CONTENT=17912; NODED=18911; HUB=19911; INBOUND=19095; E2B=18982; MCP=18128
for p in $API $CONTENT $NODED $HUB $INBOUND $E2B $MCP; do
  if nc -z 127.0.0.1 "$p" 2>/dev/null; then echo "e2e-mac: port $p is in use; this run needs it free"; exit 1; fi
done

ROOT=$(mktemp -d /private/tmp/apz-e2e.XXXXXX)
STATE=$ROOT/state; HOME_DIR=$ROOT/home; LOGS=$ROOT/logs; UDS=$ROOT/uds
mkdir -p "$STATE" "$HOME_DIR" "$LOGS" "$UDS"
: > "$ROOT/empty.env"
PIDS=()
cleanup() {
  for pid in "${PIDS[@]}"; do kill -TERM "$pid" 2>/dev/null || true; done
  sleep 2
  # The VMs (apz-vmm) and the weft server apid starts carry this run's directory in their arguments.
  pkill -TERM -f "$ROOT" 2>/dev/null || true
  sleep 1
  if pgrep -f "$ROOT" >/dev/null 2>&1; then echo "e2e-mac: processes left under $ROOT"; fi
  if [ "${APZ_E2E_KEEP:-}" = 1 ]; then echo "e2e-mac: kept $ROOT"; else rm -rf "$ROOT"; fi
}
trap cleanup EXIT

# Signed for the hypervisor (ad hoc, the same signing the packaged app's apz-vmm has).
scripts/hvf-run.sh --sign "$BIN/apz-vmm"

now_ms() { python3 -c 'import time; print(int(time.time() * 1000))'; }
# Waits up to $2 seconds for the line $3 in file $1; prints the elapsed ms when it appears.
wait_line() {
  local file=$1 limit=$2 pattern=$3 start; start=$(now_ms)
  while [ $(( $(now_ms) - start )) -lt $(( limit * 1000 )) ]; do
    if [ -f "$file" ] && grep -q "$pattern" "$file"; then return 0; fi
    sleep 0.2
  done
  echo "e2e-mac: no '$pattern' in $file within ${limit}s"; tail -20 "$file" 2>/dev/null || true
  return 1
}
json() { curl -s -m 5 "127.0.0.1:$API$1"; }

T0=$(now_ms)
export HOME=$HOME_DIR
CIDS=$(seq -s, 20 131)
"$BIN/apid" --state-dir "$STATE" --env-file "$ROOT/empty.env" --port $API --noded-port $NODED \
  --hub-port $HUB --e2b-gateway-port $E2B --mcp-proxy-port $MCP --roles docs/agents/initial-roles.json \
  --prompts docs/codex-draft/roles --vault-key "$ROOT/vault.key" --dev-models > "$LOGS/apid.log" 2>&1 &
PIDS+=($!)
wait_line "$LOGS/apid.log" 60 "apid: listening on" || exit 1
T_API=$(( $(now_ms) - T0 ))

"$BIN/vsockd" --uds-dir "$UDS" --cids "$CIDS" --port 5000 --http-port $HUB --e2b-port $E2B \
  --apid "http://127.0.0.1:$API" --internal-token-file "$STATE/internal.token" > "$LOGS/vsockd.log" 2>&1 &
PIDS+=($!)
wait_line "$LOGS/vsockd.log" 60 "http on" || exit 1

"$BIN/noded" --port $NODED --uds-dir "$UDS" --agents docs/agents/initial-roles.json \
  --kernel "$PWD/$GUEST/vmlinux" --initrd "$PWD/$GUEST/initrd.gz" \
  --internal-token-file "$STATE/internal.token" > "$LOGS/noded.log" 2>&1 &
PIDS+=($!)
wait_line "$LOGS/noded.log" 60 "listening on" || exit 1

# The first screen's checks ("this machine"): the overview and the providers the setup asks for.
json /v1/overview > /dev/null
json /v1/providers > /dev/null

# The orchestrator's VM connects to the hub: the agents list shows it idle.
ready=0
for _ in $(seq 1 600); do
  # Ready: an idle agent that the plaza knows as the orchestrator (its role comes from noded's run report,
  # a moment after its hello).
  found=$(json /v1/agents | python3 -c '
import json,sys
agents = json.load(sys.stdin)
print(int(any(a.get("role") == "orchestrator" and a.get("status") == "idle" for a in agents)))' 2>/dev/null || echo 0)
  if [ "${found:-0}" = 1 ]; then ready=1; break; fi
  sleep 0.2
done
[ $ready = 1 ] || { echo "e2e-mac: the orchestrator did not connect"; tail -20 "$LOGS/noded.log"; exit 1; }
T_READY=$(( $(now_ms) - T0 ))

# The chat with the main orchestrator: one message, the answer from its VM.
T_SEND=$(now_ms)
json /v1/chats/main > /dev/null
curl -s -m 5 -X POST "127.0.0.1:$API/v1/chats/main/messages" -H 'content-type: application/json' \
  -d '{"text":"Hello orchestrator, say hi in one sentence.","attachments":[]}' > /dev/null
answer=""
for _ in $(seq 1 600); do
  answer=$(json /v1/chats/main | python3 -c '
import json,sys
chat=json.load(sys.stdin)
for m in chat.get("messages", []):
    if m.get("author") == "session" and m.get("seat") == "orchestrator":
        print(m.get("text", "")); break
' 2>/dev/null || true)
  [ -n "$answer" ] && break
  sleep 0.2
done
[ -n "$answer" ] || { echo "e2e-mac: no answer from the orchestrator"; tail -30 "$LOGS/apid.log"; exit 1; }
T_ANSWER=$(now_ms)

echo "e2e-mac: answer: $answer"
echo "e2e-mac: timing (ms): apid listening $T_API, orchestrator VM connected $T_READY, answer $(( T_ANSWER - T0 )) from start (reply $(( T_ANSWER - T_SEND )) after the message)"
total=$(( T_ANSWER - T0 ))
if [ "$total" -lt 60000 ]; then
  echo "e2e-mac: PASS: the chat with the orchestrator in ${total} ms (under a minute)"
else
  echo "e2e-mac: FAIL: the chat took ${total} ms (over a minute)"; exit 1
fi
