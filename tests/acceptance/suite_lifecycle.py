"""Suite: lifecycle — how fast an agent VM sleeps, wakes, checkpoints and forks, and what an idle agent costs.

The numbers the scale work is judged by, measured end to end on this host and recorded per machine class in
``tests/acceptance/results/``:

* **Wake** — one keepalive agent (``noded --agents``) is put to sleep and woken ``WAKE_CYCLES`` times. A wake is
  timed from the ``POST .../wake`` to the guest's hello reaching the hub (vsockd's ``agent <cid> role <role>
  connected`` line), so it covers the node's restore *and* the guest's reconnect. Sleep is timed as the
  ``POST .../sleep`` call. noded's own ``timing op=wake`` lines are kept as the node-side breakdown.
* **Checkpoint and fork** — ``CHECKPOINTS`` checkpoints of the agent (the API call), and ``FORKS`` forks of two
  copies each, timed from the ``POST .../fork`` to both copies' hellos.
* **Idle density** — ``--soak N`` idle keepalive agents held for ``--soak-seconds``: host RSS per VM, the daemons'
  and VMMs' CPU, their threads, and vsockd's session wakeups per second (``/v1/metrics``), which is the idle cost
  per agent the event-driven sessions are meant to keep near one wake per heartbeat.

Nothing here asserts a speed: every number is measured and recorded, and a p95 regression against the previous
result for the same machine class is reported as a finding (like the budgets suite).
"""

from __future__ import annotations

import json
import os
import re
import time
from pathlib import Path

import apz
from apz import Lab, Suite, percentile, summary

#: The keepalive agents' guest CIDs start here (``noded --agents-cid-base``).
AGENT_CID_BASE = 100
#: Fork copies get CIDs from noded's run range (``--run-cids``, default 20-99); vsockd listens on these too.
FORK_CIDS = list(range(20, 40))
#: Sleep/wake cycles of one agent.
WAKE_CYCLES = 20
#: Checkpoints of one agent.
CHECKPOINTS = 10
#: Forks of two copies each.
FORKS = 5
#: How long a guest may take to reach the hub (first boot of a fleet, a cold fallback).
GUEST_SETTLE_S = 90.0
#: How long one wake or fork may take before it counts as failed.
OP_DEADLINE_S = 30.0
#: How often a wait looks at the logs.
POLL_S = 0.002


def _manifest(ctx, roles: list[str], tag: str) -> Path:
    """A role manifest for ``noded --agents`` with one keepalive agent per role."""
    path = ctx.new_dir(f"lifecycle-manifest-{tag}") / "roles.json"
    body = {
        "source": "tests/acceptance/suite_lifecycle.py",
        "roles": {
            role: {"role": role, "title": f"The {role}", "permits": [], "toolsets": [], "budget": 1, "variant": None}
            for role in roles
        },
    }
    path.write_text(json.dumps(body, indent=2) + "\n")
    return path


def _agent_lab(ctx, roles: list[str], tag: str, extra_cids: list[int] | None = None) -> Lab:
    """A lab whose noded boots one keepalive agent per role (CIDs from ``AGENT_CID_BASE``), keeps snapshots in the
    lab's own work dir, and whose vsockd also listens on ``extra_cids``. Not started: use it as a context manager."""
    cids = [AGENT_CID_BASE + index for index in range(len(roles))] + list(extra_cids or [])
    snapshots = ctx.new_dir(f"lifecycle-snapshots-{tag}")
    lab = Lab(
        ctx,
        cids,
        f"lifecycle-{tag}",
        agents=_manifest(ctx, roles, tag),
        agents_cid_base=AGENT_CID_BASE,
        noded_args=["--snapshot-dir", str(snapshots)],
    )
    # noded boots the whole role fleet before it serves: allow for each agent's boot.
    lab.start_deadline_s = apz.START_DEADLINE_S + 4.0 * len(roles)
    return lab


def _fleet_run(lab: Lab) -> str | None:
    """The run id of the initial role fleet, from noded's startup line."""
    for line in lab.noded_log_text().splitlines():
        match = re.search(r"noded: initial role fleet (\S+):", line)
        if match:
            return match.group(1)
    return None


def _connected_count(lab: Lab, cid: int) -> int:
    """How many times vsockd has logged guest ``cid`` connecting (one line per hello it registered)."""
    needle = f"vsockd: agent {cid} claimed role "
    return sum(1 for line in lab.vsockd_log_text().splitlines() if needle in line and line.endswith("connected"))


def _await_connected(lab: Lab, cid: int, after: int, deadline_s: float) -> bool:
    """Waits until vsockd has logged more than ``after`` connects of ``cid``; False at the deadline."""
    end = time.monotonic() + deadline_s
    while time.monotonic() < end:
        if _connected_count(lab, cid) > after:
            return True
        time.sleep(POLL_S)
    return False


def _vm_index(lab: Lab, run_id: str, cid: int, role: str = "builder") -> int | None:
    """The index of the VM whose guest CID is ``cid`` in run ``run_id``; a fleet VM's view may carry only its role,
    so a VM with no CID is matched by ``role``."""
    vms = lab.api_get(f"/v1/runs/{run_id}").json.get("vms", [])
    for vm in vms:
        if vm.get("guest_cid") == cid:
            return vm.get("id")
    for vm in vms:
        if vm.get("guest_cid") is None and vm.get("role") == role:
            return vm.get("id")
    return None


def _timing_lines(lab: Lab, op: str) -> list[dict]:
    """noded's ``timing op=<op>`` lines, parsed into dicts of their ``key=value`` fields."""
    rows = []
    for line in lab.noded_log_text().splitlines():
        if line.startswith(f"timing op={op} "):
            rows.append(dict(field.split("=", 1) for field in line.split()[1:] if "=" in field))
    return rows


def _case_wake(suite: Suite, ctx) -> None:
    case = suite.case(
        "sleep and wake: one keepalive agent slept and woken repeatedly, timed until its hello reaches the hub",
        vector=f"noded --agents (1 role); {WAKE_CYCLES} x (POST sleep, POST wake, wait for vsockd's connected line)",
    )
    cid = AGENT_CID_BASE
    with _agent_lab(ctx, ["builder"], "wake") as lab:
        run_id = _fleet_run(lab)
        if not case.invariant("noded booted the fleet run", run_id is not None, lab.noded_log_text()[-400:]):
            return
        if not case.invariant(
            "the agent reached the hub", _await_connected(lab, cid, 0, GUEST_SETTLE_S), lab.vsockd_log_text()[-400:]
        ):
            return
        index = _vm_index(lab, run_id, cid)
        sleeps, wakes, wake_calls, failures = [], [], [], []
        for cycle in range(WAKE_CYCLES):
            before = _connected_count(lab, cid)
            started = time.monotonic()
            slept = lab.api_post(f"/v1/runs/{run_id}/vms/{index}/sleep", {})
            sleeps.append((time.monotonic() - started) * 1000.0)
            if slept.status >= 300:
                failures.append(f"cycle {cycle}: sleep answered {slept.status}: {slept.body[:200]}")
                break
            started = time.monotonic()
            woke = lab.api_post(f"/v1/runs/{run_id}/vms/{index}/wake", {})
            wake_calls.append((time.monotonic() - started) * 1000.0)
            if woke.status >= 300:
                failures.append(f"cycle {cycle}: wake answered {woke.status}: {woke.body[:200]}")
                break
            if not _await_connected(lab, cid, before, OP_DEADLINE_S):
                failures.append(f"cycle {cycle}: no hello within {OP_DEADLINE_S}s")
                break
            wakes.append((time.monotonic() - started) * 1000.0)
        case.invariant("every cycle slept and woke", not failures, "; ".join(failures))
        node = [float(row["total_ms"]) for row in _timing_lines(lab, "wake") if "total_ms" in row]
        suite.data["wake"] = {
            "wake_to_hello_ms": summary(wakes),
            "wake_call_ms": summary(wake_calls),
            "samples": {"wake_to_hello_ms": wakes, "wake_call_ms": wake_calls, "sleep_ms": sleeps},
            "sleep_ms": summary(sleeps),
            "node_wake_ms": summary(node),
            "cold_fallbacks": sum(1 for row in _timing_lines(lab, "wake") if row.get("fallback") == "true"),
        }
        case.note(
            f"wake to hello p50={percentile(wakes, 0.5)} p95={percentile(wakes, 0.95)} ms; "
            f"node part p50={percentile(node, 0.5)} ms; sleep p50={percentile(sleeps, 0.5)} ms"
        )


def _case_checkpoint_fork(suite: Suite, ctx) -> None:
    """One keepalive agent is checkpointed ``CHECKPOINTS`` times (the API call is timed), then forked ``FORKS`` times,
    two copies each, timed from the fork call until both copies' hellos reach the hub. Each fork's run is deleted
    after its timing. noded's own ``timing`` lines for op=checkpoint and op=fork are kept as the node-side breakdown."""
    case = suite.case(
        "checkpoint and fork: one keepalive agent checkpointed and forked, forks timed until both copies' hellos",
        vector=f"noded --agents (1 role); {CHECKPOINTS} x POST checkpoints; {FORKS} x POST fork {{n: 2}} "
        "(timed until both copies' hellos)",
    )
    cid = AGENT_CID_BASE
    with _agent_lab(ctx, ["builder"], "fork", extra_cids=FORK_CIDS) as lab:
        run_id = _fleet_run(lab)
        if not case.invariant("noded booted the fleet run", run_id is not None, lab.noded_log_text()[-400:]):
            return
        if not case.invariant(
            "the agent reached the hub", _await_connected(lab, cid, 0, GUEST_SETTLE_S), lab.vsockd_log_text()[-400:]
        ):
            return
        index = _vm_index(lab, run_id, cid)
        checkpoint_ms, fork_ms, failures = [], [], []
        for i in range(CHECKPOINTS):
            started = time.monotonic()
            made = lab.api_post(f"/v1/runs/{run_id}/vms/{index}/checkpoints", {"label": f"bench-{i}", "memory": True})
            checkpoint_ms.append((time.monotonic() - started) * 1000.0)
            if made.status != 201:
                failures.append(f"checkpoint {i} answered {made.status}: {made.body[:200]}")
                break
        for i in range(FORKS):
            before = {fork_cid: _connected_count(lab, fork_cid) for fork_cid in FORK_CIDS}
            started = time.monotonic()
            forked = lab.api_post(f"/v1/runs/{run_id}/vms/{index}/fork", {"n": 2})
            if forked.status != 201:
                failures.append(f"fork {i} answered {forked.status}: {forked.body[:200]}")
                break
            try:
                forks = json.loads(forked.body).get("forks", [])
            except (ValueError, AttributeError):
                forks = []
            connected = bool(forks) and all(
                _await_connected(lab, fork["cid"], before.get(fork["cid"], 0), OP_DEADLINE_S) for fork in forks
            )
            elapsed = (time.monotonic() - started) * 1000.0
            for fork in forks:
                lab.api_delete(f"/v1/runs/{fork['run']}")
            if not connected:
                failures.append(f"fork {i}: a copy had no hello within {OP_DEADLINE_S}s")
                break
            fork_ms.append(elapsed)
        case.invariant("every checkpoint and fork succeeded", not failures, "; ".join(failures))
        checkpoint_node = [float(row["total_ms"]) for row in _timing_lines(lab, "checkpoint") if "total_ms" in row]
        fork_node = [float(row["total_ms"]) for row in _timing_lines(lab, "fork") if "total_ms" in row]
        suite.data["checkpoint"] = {"call_ms": summary(checkpoint_ms), "node_ms": summary(checkpoint_node)}
        suite.data["fork"] = {"to_hellos_ms": summary(fork_ms), "node_ms": summary(fork_node)}
        case.note(
            f"checkpoint call p50={percentile(checkpoint_ms, 0.5)} p95={percentile(checkpoint_ms, 0.95)} ms; "
            f"fork to hellos p50={percentile(fork_ms, 0.5)} p95={percentile(fork_ms, 0.95)} ms; "
            f"node fork part p50={percentile(fork_node, 0.5)} ms"
        )


def _voluntary_switches(lab: Lab) -> int:
    """Voluntary context switches summed over every thread of every process in the lab's process group."""
    total = 0
    for pid in lab.process_group_members():
        try:
            tids = os.listdir(f"/proc/{pid}/task")
        except OSError:
            continue
        for tid in tids:
            try:
                text = Path(f"/proc/{pid}/task/{tid}/status").read_text()
            except OSError:
                continue
            for line in text.splitlines():
                if line.startswith("voluntary_ctxt_switches:"):
                    total += int(line.split()[1])
                    break
    return total


def _case_density(suite: Suite, ctx, agents: int, seconds: float) -> None:
    """`agents` keepalive agents (roles "a0".."aN"), wait for all hellos, then hold idle for `seconds`, sampling the
    host-side cost at the start and end of the window; records per-agent RSS, CPU % of one core, threads per agent
    and voluntary context switches (wakeups) per second per agent in suite.data["density"]."""
    roles = [f"a{i}" for i in range(agents)]
    cids = [AGENT_CID_BASE + i for i in range(agents)]
    case = suite.case(
        f"idle density: {agents} keepalive agents held idle for {seconds:.0f}s",
        vector=f"noded --agents ({agents} roles); all hellos awaited, then a {seconds:.0f}s idle window between samples",
    )
    with _agent_lab(ctx, roles, f"density-{agents}") as lab:
        deadline = time.monotonic() + GUEST_SETTLE_S + agents * 2
        while time.monotonic() < deadline:
            if all(_connected_count(lab, cid) >= 1 for cid in cids):
                break
            time.sleep(0.5)
        reached = sum(1 for cid in cids if _connected_count(lab, cid) >= 1)
        if not case.invariant("every agent reached the hub", reached == agents, f"{reached} of {agents} reached the hub"):
            return
        time.sleep(10)
        rss_start, ticks_start = lab.rss(), lab.cpu_ticks()
        threads_start, switches_start = lab.threads(), _voluntary_switches(lab)
        time.sleep(seconds)
        rss_end, ticks_end = lab.rss(), lab.cpu_ticks()
        threads_end, switches_end = lab.threads(), _voluntary_switches(lab)
        # The doze tier: every VM paused and paged out (noded `.../doze`), then the host cost again. A noded
        # without the route (a build before the doze tier) skips this part.
        dozed = _doze_all(lab)
        rss_dozed = None
        if dozed:
            time.sleep(5)
            rss_dozed = lab.rss()
    mib = 1024 * 1024
    clk_tck = os.sysconf("SC_CLK_TCK")
    rss_mib_per_agent = rss_end / agents / mib
    cpu_pct_one_core = (ticks_end - ticks_start) / clk_tck / seconds * 100
    threads_per_agent = threads_end / agents
    wakeups_per_s_per_agent = (switches_end - switches_start) / seconds / agents
    suite.data["density"] = {
        "agents": agents,
        "seconds": seconds,
        "rss_mib_start": round(rss_start / mib, 3),
        "rss_mib_per_agent": round(rss_mib_per_agent, 3),
        "cpu_pct_one_core": round(cpu_pct_one_core, 3),
        "threads_start": threads_start,
        "threads_per_agent": round(threads_per_agent, 3),
        "wakeups_per_s_per_agent": round(wakeups_per_s_per_agent, 3),
        "dozed": dozed,
        "rss_mib_per_agent_dozing": None if rss_dozed is None else round(rss_dozed / agents / mib, 3),
    }
    case.note(
        f"{agents} agents idle {seconds:.0f}s: rss {rss_mib_per_agent:.2f} MiB/agent; "
        f"cpu {cpu_pct_one_core:.2f}% of one core; {threads_per_agent:.2f} threads/agent; "
        f"{wakeups_per_s_per_agent:.2f} voluntary context switches/s/agent"
    )
    case.invariant("the measurements were taken", True, "")


def _doze_all(lab: Lab) -> int:
    """Dozes every VM of the lab's role fleet; how many dozed (0 when noded has no doze route)."""
    run_id = _fleet_run(lab)
    if run_id is None:
        return 0
    runs = lab.api_get("/v1/runs").json.get("runs", [])
    vms = [vm for run in runs if run.get("id") == run_id for vm in run.get("vms", [])]
    dozed = 0
    for vm in vms:
        reply = lab.api_post(f"/v1/runs/{run_id}/vms/{vm['id']}/doze", {})
        if reply.status != 200:
            return dozed
        dozed += 1
    return dozed


def _case_sessions(suite: Suite, ctx, sessions: int, seconds: float) -> None:
    """One vsockd serving `sessions` CIDs, each with a connected keepalive guest stood in by a socket that says hello and
    heartbeats every 5 s (no VMs: this measures the gateway alone). Records vsockd's CPU, threads and RSS while it holds
    them, and that none was refused, dropped or reaped, in suite.data["sessions"]."""
    import selectors
    import socket
    import subprocess

    case = suite.case(
        f"connected sessions: one vsockd holds {sessions} keepalive guests for {seconds:.0f}s",
        vector=f"vsockd --cids <{sessions}> --max-sessions {max(4096, sessions)}; a hello and a heartbeat every 5 s per CID",
    )
    work = ctx.new_dir(f"lifecycle-sessions-{sessions}")
    uds = work / "uds"
    uds.mkdir(mode=0o700)
    cids = list(range(1000, 1000 + sessions))
    http = apz._free_port()
    log_path = work / "vsockd.log"
    with open(log_path, "w") as log:
        daemon = subprocess.Popen(
            [
                str(ctx.bin("vsockd")), "--uds-dir", str(uds), "--cids", ",".join(map(str, cids)),
                "--port", str(apz.VSOCK_PORT), "--http-port", str(http),
                "--apid", f"http://127.0.0.1:{apz._free_port()}", "--max-sessions", str(max(4096, sessions)),
            ],
            stdout=log,
            stderr=subprocess.STDOUT,
        )
    conns: list[socket.socket] = []
    try:
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            if sum(1 for name in os.listdir(uds) if name.endswith(f"_{apz.VSOCK_PORT}")) >= sessions:
                break
            time.sleep(0.5)
        selector = selectors.DefaultSelector()
        welcomed: set[int] = set()
        for cid in cids:
            conn = socket.socket(socket.AF_UNIX)
            conn.connect(str(uds / f"vm{cid}_{apz.VSOCK_PORT}"))
            hello = {"type": "hello", "vm": 0, "guest_cid": cid, "role": "soak", "keepalive": True}
            conn.sendall((json.dumps(hello) + "\n").encode())
            conn.setblocking(False)
            selector.register(conn, selectors.EVENT_READ, cid)
            conns.append(conn)
        deadline = time.monotonic() + 60
        while len(welcomed) < sessions and time.monotonic() < deadline:
            for key, _ in selector.select(1):
                try:
                    if b"welcome" in key.fileobj.recv(65536):
                        welcomed.add(key.data)
                except BlockingIOError:
                    pass
        if not case.invariant("every guest was welcomed", len(welcomed) == sessions, f"{len(welcomed)} of {sessions}"):
            return

        def sample() -> tuple[int, int, int]:
            fields = Path(f"/proc/{daemon.pid}/stat").read_text().split(")")[1].split()
            rss_kib = next(
                int(line.split()[1])
                for line in Path(f"/proc/{daemon.pid}/status").read_text().splitlines()
                if line.startswith("VmRSS:")
            )
            return int(fields[11]) + int(fields[12]), len(os.listdir(f"/proc/{daemon.pid}/task")), rss_kib

        time.sleep(3)
        ticks_start, _, _ = sample()
        start = time.monotonic()
        next_beat = start
        seq = 0
        while time.monotonic() - start < seconds:
            if time.monotonic() >= next_beat:
                seq += 1
                beat = (json.dumps({"type": "heartbeat", "seq": seq}) + "\n").encode()
                for conn in conns:
                    try:
                        conn.sendall(beat)
                    except BlockingIOError:
                        pass
                next_beat += 5
            for key, _ in selector.select(0.5):
                try:
                    key.fileobj.recv(65536)
                except BlockingIOError:
                    pass
        ticks_end, threads, rss_kib = sample()
        cpu = (ticks_end - ticks_start) / os.sysconf("SC_CLK_TCK") / seconds * 100
        lost = sum(
            1 for line in log_path.read_text().splitlines() if "dropped" in line or "reaped" in line or "refusing" in line
        )
        suite.data["sessions"] = {
            "sessions": sessions,
            "seconds": seconds,
            "cpu_pct_one_core": round(cpu, 3),
            "threads": threads,
            "rss_mib": round(rss_kib / 1024, 1),
        }
        case.invariant("no session was refused, dropped or reaped", lost == 0, f"{lost} such lines in vsockd.log")
        case.note(f"{sessions} sessions for {seconds:.0f}s: cpu {cpu:.2f}% of one core, {threads} threads, rss {rss_kib // 1024} MiB")
    finally:
        for conn in conns:
            conn.close()
        daemon.terminate()
        daemon.wait(timeout=20)


def run(ctx, soak: int = 0, soak_seconds: float = 60.0, soak_sessions: int = 0) -> Suite:
    suite = Suite(
        "lifecycle",
        "sleep, wake, checkpoint and fork measured end to end (until the guest's hello), and the idle cost per agent "
        "with --soak; numbers are recorded, not asserted",
        ctx,
    )
    reason = ctx.can_run("lifecycle")
    if reason:
        suite.blocked(reason)
        return suite
    _case_wake(suite, ctx)
    _case_checkpoint_fork(suite, ctx)
    if soak > 0:
        _case_density(suite, ctx, soak, soak_seconds)
    if soak_sessions > 0:
        _case_sessions(suite, ctx, soak_sessions, soak_seconds)
    return suite
