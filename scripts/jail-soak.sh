#!/usr/bin/env bash
# The jail's soak: a real fleet under the armed seccomp filter for MINUTES.
#
#   scripts/jail-soak.sh [minutes]      # default 10
#
# What it does. Boots `noded --agents <3 real keepalive guests>` and a `vsockd`
# hub exactly the way the live deployment does, waits for all three guests to
# connect, and then churns the paths the live fleet was killed on: host-initiated
# connections to each VM's vsock muxer socket (half-open and RST — the muxer's
# `local-init` read, `recvfrom`), a hub-driven tool call and session stop
# (`bye`), and a workspace write over a guest's serial console (virtio-blk). It
# then asserts the daemon is still the same process at the end and that the
# kernel recorded no seccomp refusal during the run.
#
# Exit status: 0 survived with no refusal, 1 died or was refused, 2 the
# environment is not ready (build the workspace first:
# `cargo build --workspace && scripts/mk-initrd-x86_64.sh`).
#
# Env:
#   APZ_SOAK_JAIL      the daemon's jail mode: kill (default), log or off
#   APZ_SOAK_KERNEL    guest kernel (default /boot/vmlinuz-linux)
#   APZ_SOAK_INITRD    guest initramfs (default target/guest/initrd.x86_64.gz)
#   APZ_SOAK_WORKSPACE base image of the writable workspace volume
#                      (default target/guest/workspace.ext4; skipped if absent)
#   APZ_SOAK_BIN       where noded/vsockd live (default target/debug)
#
# `off` is the control that shows the churn itself is harmless; `log` shows the
# audit arm keeping a daemon whose allowlist was wrong alive; `kill` is the
# shipped default and the one that must not die.
set -euo pipefail
cd "$(dirname "$0")/.."

minutes="${1:-10}"
jail_mode="${APZ_SOAK_JAIL:-kill}"
kernel="${APZ_SOAK_KERNEL:-/boot/vmlinuz-linux}"
initrd="${APZ_SOAK_INITRD:-target/guest/initrd.x86_64.gz}"
workspace="${APZ_SOAK_WORKSPACE:-target/guest/workspace.ext4}"
bin_dir="${APZ_SOAK_BIN:-target/debug}"

for f in "$bin_dir/noded" "$bin_dir/vsockd" "$kernel" "$initrd"; do
    if [ ! -e "$f" ]; then
        echo "jail-soak: missing $f (cargo build --workspace && scripts/mk-initrd-x86_64.sh)" >&2
        exit 2
    fi
done
case "$jail_mode" in
    kill|log|off) ;;
    *) echo "jail-soak: APZ_SOAK_JAIL=$jail_mode is not kill, log or off" >&2; exit 2 ;;
esac

work="$(mktemp -d /tmp/apz-jail-soak-XXXXXX)"

# Stop the soak's own daemons however the body ends — the body's tail stops
# them on a clean finish, and this covers a failure or a signal. The match is
# the work directory the body gave them, which nothing else in the project
# uses: a bare `kill 0` here would signal whatever process group the caller
# happens to share with this script.
cleanup() {
    pkill -f "uds-dir $work/uds" 2>/dev/null || true
}
trap 'cleanup; echo "jail-soak: work dir $work"' EXIT

python3 - "$work" "$minutes" "$jail_mode" "$bin_dir/noded" "$bin_dir/vsockd" "$kernel" "$initrd" "$workspace" <<'PY'
"""The soak body: boot the fleet, churn it, and report what survived."""
import json
import os
import re
import signal
import socket
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

work, minutes, jail_mode, noded_bin, vsockd_bin, kernel, initrd, workspace = sys.argv[1:9]
work = Path(work)
minutes = float(minutes)
uds = work / "uds"
uds.mkdir(parents=True, exist_ok=True)
os.chmod(uds, 0o700)
logs = work / "logs"
logs.mkdir(exist_ok=True)

ROLES = ("architect", "builder", "reviewer")   # alphabetical: cids 100, 101, 102
CID_BASE, VSOCK_PORT = 100, 5000
CIDS = [CID_BASE + i for i in range(len(ROLES))]

manifest = work / "roles.json"
manifest.write_text(json.dumps({"source": "scripts/jail-soak.sh", "roles": {
    r: {"role": r, "title": f"The {r}", "permits": [], "toolsets": [], "budget": 1, "variant": None}
    for r in ROLES}}, indent=2) + "\n")


def say(msg):
    print(f"jail-soak: {msg}", flush=True)


def await_line(path, needle, timeout, proc):
    deadline = time.time() + timeout
    while time.time() < deadline:
        text = path.read_text(errors="replace") if path.exists() else ""
        for line in text.splitlines():
            if needle in line:
                return line
        if proc.poll() is not None:
            raise SystemExit(f"jail-soak: {path.name} exited before {needle!r}:\n{text[-2000:]}")
        time.sleep(0.1)
    raise SystemExit(f"jail-soak: timeout waiting for {needle!r} in {path.name}")


noded_log, vsockd_log = logs / "noded.log", logs / "vsockd.log"
vsockd = subprocess.Popen(
    [vsockd_bin, "--uds-dir", str(uds), "--cids", ",".join(map(str, CIDS)),
     "--port", str(VSOCK_PORT), "--status-port", "0", "--admin-port", "0"],
    stdout=open(vsockd_log, "wb"), stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL,
    start_new_session=True)
await_line(vsockd_log, "listening on", 30, vsockd)
admin_port = int(re.search(r"127\.0\.0\.1:(\d+)",
                           await_line(vsockd_log, "admin on http", 30, vsockd)).group(1))

noded_env = dict(os.environ)
# The mode under test reaches the daemon's VMM threads through its environment.
if jail_mode == "off":
    noded_env["APZ_VMM_NO_JAIL"] = "1"
    noded_env.pop("APZ_VMM_JAIL", None)
else:
    noded_env["APZ_VMM_JAIL"] = jail_mode
    noded_env.pop("APZ_VMM_NO_JAIL", None)

argv = [noded_bin, "--port", "0", "--uds-dir", str(uds), "--kernel", kernel,
        "--initrd", initrd, "--agents", str(manifest), "--agents-cid-base", str(CID_BASE),
        "--hub-admin", f"127.0.0.1:{admin_port}", "--hub-status", "off"]
workspace_path = Path(workspace)
if workspace_path.exists():
    argv += ["--workspace", str(workspace_path)]
say(f"booting noded (APZ_VMM_JAIL={jail_mode if jail_mode != 'off' else 'off (control)'})")
noded = subprocess.Popen(argv, stdout=open(noded_log, "wb"), stderr=subprocess.STDOUT,
                         stdin=subprocess.DEVNULL, start_new_session=True, env=noded_env)
line = await_line(noded_log, "listening on", 180, noded)
port = int(re.search(r"127\.0\.0\.1:(\d+)", line).group(1))
say(f"noded pid {noded.pid} on port {port}")


def connected():
    text = vsockd_log.read_text(errors="replace")
    return sum(1 for i, r in enumerate(ROLES) if f"agent {CID_BASE+i} role {r} connected" in text)


deadline = time.time() + 240
while time.time() < deadline and connected() < len(ROLES):
    if noded.poll() is not None:
        raise SystemExit(f"jail-soak: noded died while the guests booted (rc={noded.returncode})")
    time.sleep(0.5)
say(f"{connected()}/{len(ROLES)} guests connected")


def http(method, url, body=None, timeout=20.0):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    if data is not None:
        req.add_header("content-type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as reply:
            raw = reply.read().decode()
            return reply.status, (json.loads(raw) if raw.strip() else {})
    except (urllib.error.URLError, OSError) as err:
        return 0, str(err)


def half_open_and_drop(path):
    """Connect and close with nothing written: the muxer's local-init read sees
    EOF and returns (the live fleet's `UnexpectedEof` line), and its `recvfrom`
    is the syscall that used to kill the daemon."""
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(2.0)
    try:
        s.connect(path)
    except OSError:
        pass
    finally:
        s.close()


def rst_connect(path):
    """Connect and RST it (SO_LINGER 0): an abrupt peer disconnect."""
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(2.0)
    try:
        s.connect(path)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
    except OSError:
        pass
    finally:
        s.close()


base = f"http://127.0.0.1:{port}"
try:
    run = http("GET", f"{base}/api/v0/runs")[1]["runs"][0]["id"]
except Exception as err:  # the fleet run is the daemon's own
    raise SystemExit(f"jail-soak: cannot read the fleet run: {err}")


def audit_count(pid):
    """How many seccomp audit records the kernel has for *this* daemon, or None
    when this user cannot read the kernel log (the count is evidence, not a
    gate). The pid filter is what makes the count the soak's own: any other
    process that arms a filter in this window — a test suite run beside it —
    records its own pid and is not counted here."""
    try:
        text = subprocess.run(["journalctl", "-k", "--no-pager", "-n", "200000"],
                              capture_output=True, text=True, timeout=60).stdout
    except (OSError, subprocess.TimeoutExpired):
        return None
    return sum(1 for line in text.splitlines()
               if "type=1326" in line and f"pid={pid} " in line)


before = audit_count(noded.pid)
say(f"seccomp audit records for pid {noded.pid} before the soak: "
    f"{before if before is not None else 'unreadable'}")

started = time.time()
tells = stops = wsrites = churns = 0
cycle = 0
died_after = None
while time.time() - started < minutes * 60:
    cycle += 1
    for cid in CIDS:
        half_open_and_drop(str(uds / f"vm{cid}"))
        churns += 1
    rst_connect(str(uds / f"vm{CID_BASE}"))
    half_open_and_drop(str(uds / f"vm{CID_BASE}_{VSOCK_PORT}"))
    if noded.poll() is not None:
        died_after = time.time() - started
        break
    # A hub-driven tool call into a connected agent.
    if cycle % 3 == 1:
        status, _ = http("POST", f"{base}/api/v0/agents/{CID_BASE}/tell",
                         {"name": "echo", "args": {"text": f"soak-{cycle}"}})
        tells += status == 200
    # A workspace write over a guest's serial console (virtio-blk on the vCPU).
    if cycle % 4 == 2:
        status, _ = http("POST", f"{base}/api/v0/runs/{run}/input",
                         {"vm": 0, "data": f"wspace soak-{cycle}\n"})
        wsrites += status == 200
    # The hub's stop view: the agent answers `bye`, the session drains and the
    # guest's harness reconnects — session churn of the kind the live fleet saw.
    if cycle % 45 == 0:
        status, _ = http("POST", f"{base}/api/v0/agents/{CID_BASE + 2}/stop",
                         {"reason": "jail-soak"})
        stops += status == 200
    time.sleep(1.0)

elapsed = time.time() - started
alive = noded.poll() is None
after = audit_count(noded.pid)
new_records = None if (before is None or after is None) else after - before

say(f"soak: {elapsed:.0f}s, {cycle} cycles, {churns} muxer churns, "
    f"{tells} tool calls, {wsrites} workspace writes, {stops} session stops")
say(f"guests connected at the end: {connected()}/{len(ROLES)}; noded alive: {alive}; "
    f"seccomp records for pid {noded.pid}: {before} -> {after}")

if not alive:
    say(f"FAIL: the daemon died after {died_after:.0f}s (rc={noded.returncode})")
    print(noded_log.read_text(errors="replace")[-2000:])
    sys.exit(1)
if new_records not in (None, 0):
    say(f"FAIL: the kernel recorded {new_records} seccomp refusals for pid {noded.pid}")
    sys.exit(1)
if connected() < len(ROLES):
    say("FAIL: the fleet was not still connected at the end")
    sys.exit(1)

say(f"PASS: no death and no seccomp record in {elapsed/60:.1f} minutes "
    f"under APZ_VMM_JAIL={jail_mode}")

for proc in (noded, vsockd):
    try:
        os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
    except ProcessLookupError:
        pass
for proc in (noded, vsockd):
    try:
        proc.wait(timeout=20)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
        except ProcessLookupError:
            pass
PY
