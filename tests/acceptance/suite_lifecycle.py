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
import re
import time
from pathlib import Path

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
    return Lab(
        ctx,
        cids,
        f"lifecycle-{tag}",
        agents=_manifest(ctx, roles, tag),
        agents_cid_base=AGENT_CID_BASE,
        noded_args=["--snapshot-dir", str(snapshots)],
    )


def _fleet_run(lab: Lab) -> str | None:
    """The run id of the initial role fleet, from noded's startup line."""
    for line in lab.noded_log_text().splitlines():
        match = re.search(r"noded: initial role fleet (\S+):", line)
        if match:
            return match.group(1)
    return None


def _connected_count(lab: Lab, cid: int) -> int:
    """How many times vsockd has logged guest ``cid`` connecting (one line per hello it registered)."""
    needle = f"vsockd: agent {cid} role "
    return sum(1 for line in lab.vsockd_log_text().splitlines() if needle in line and line.endswith("connected"))


def _await_connected(lab: Lab, cid: int, after: int, deadline_s: float) -> bool:
    """Waits until vsockd has logged more than ``after`` connects of ``cid``; False at the deadline."""
    end = time.monotonic() + deadline_s
    while time.monotonic() < end:
        if _connected_count(lab, cid) > after:
            return True
        time.sleep(POLL_S)
    return False


def _vm_index(lab: Lab, run_id: str, cid: int) -> int | None:
    """The index of the VM whose guest CID is ``cid`` in run ``run_id``."""
    for vm in lab.get(run_id).get("vms", []):
        if vm.get("guest_cid") == cid:
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
        sleeps, wakes, failures = [], [], []
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
            "sleep_ms": summary(sleeps),
            "node_wake_ms": summary(node),
            "cold_fallbacks": sum(1 for row in _timing_lines(lab, "wake") if row.get("fallback") == "true"),
        }
        case.note(
            f"wake to hello p50={percentile(wakes, 0.5)} p95={percentile(wakes, 0.95)} ms; "
            f"node part p50={percentile(node, 0.5)} ms; sleep p50={percentile(sleeps, 0.5)} ms"
        )


def _case_checkpoint_fork(suite: Suite, ctx) -> None:
    """TODO (job L1): one keepalive agent; CHECKPOINTS x POST /v1/runs/{run}/vms/{index}/checkpoints {"label": ...}
    timed as the call; then FORKS x POST /v1/runs/{run}/vms/{index}/fork {"n": 2} timed until both copies' CIDs
    (from the answer) have a vsockd connected line. Record suite.data["checkpoint"] and suite.data["fork"] like
    _case_wake does, plus noded's timing lines for op=checkpoint and op=fork. Delete each fork's runs after timing."""
    case = suite.case("checkpoint and fork: not measured yet")
    case.blocked("job L1 has not implemented this case")


def _case_density(suite: Suite, ctx, agents: int, seconds: float) -> None:
    """TODO (job L2): `agents` keepalive agents (roles "a0".."aN"), wait for all hellos, then hold for `seconds`:
    sample lab.rss(), lab.cpu_ticks(), lab.threads() and vsockd's /v1/metrics session wakeups at the start and end
    of the window; record per-agent RSS, CPU % of one core, threads per agent, and session wakeups per second per
    agent in suite.data["density"]."""
    case = suite.case(f"idle density: {agents} agents, not measured yet")
    case.blocked("job L2 has not implemented this case")


def run(ctx, soak: int = 0, soak_seconds: float = 60.0) -> Suite:
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
    return suite
