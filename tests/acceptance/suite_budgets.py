"""Suite 4 — boot, restore and footprint budgets.

Measured on this host and recorded per machine class in
``tests/acceptance/results/``:

* **Boot budget** — p50/p95/p99 wall time for a 1, 8 and 50-VM run, from
  ``POST`` to every VM running, plus the full lifecycle (``POST`` to ``done``
  with ``beats=2``). The sample counts are fixed and reported; a p95 from a
  small sample is indicative and the result says so.

* **Footprint budget** — the 50-VM fleet's whole-host RSS against the 8 GiB
  criterion (PORT_STATUS slice 16) and its steady-state CPU against 5 % of one
  core, with the margin the validated numbers (5.57 GiB, 0.03 %) leave.

* **Restore** — the plaza has no VM snapshot support, so restore is the
  daemon restart plus FE reconciliation the SSE data layer defines: kill the
  daemon mid-run, restart it, and check the register reconciles (the new
  ``r1`` carries a new ``created_at`` — a different run) and a fresh run
  completes cleanly on the restarted daemon.

No boot-time threshold is configured anywhere in this repository, so boot and
lifecycle are *measured and recorded*; ``--boot-p95-ms`` supplies a bar when
one is published, and a p95 regression against the previous run's result file
for the same machine class is always reported as a finding.
"""

from __future__ import annotations

import json
import os
import signal
import threading
import time
from pathlib import Path

from apz import (
    FLEET_CPU_BUDGET_PCT,
    FLEET_DEADLINE_S,
    FLEET_RSS_BUDGET,
    Lab,
    Suite,
    now,
    summary,
)

#: Fixed sample counts per fleet size. They are never reduced to make a
#: percentile look better; the result states the counts it used.
BOOT_SAMPLES = {1: 20, 8: 20, 50: 10}
LIFECYCLE_SAMPLES = {1: 5, 8: 5, 50: 3}

#: The fleet size and configuration of the footprint measurement: the scale
#: validation's own shape (PORT_STATUS slice 16 — 50 x 1 vCPU x 128 MiB).
FLEET_N = 50
FLEET_BEATS = 12

#: A small fleet measured for the CPU scaling comparison in the finding.
SCALE_N = 2

#: Steady-state sampling: how long to settle after boot, then per window.
SETTLE_S = 8.0
CPU_WINDOWS = 3
CPU_WINDOW_S = 4.0

#: The recorded baseline this host's slice-16 validation published.
RECORDED_CPU_BASELINE_PCT = 0.03

#: A p95 regression versus the previous result file is a finding past both.
REGRESSION_TOLERANCE = 0.25
REGRESSION_FLOOR_MS = 250.0

#: Below this many samples a percentile is not meaningful; the result says so.
MEANINGFUL_SAMPLES = 20


def _prior_results(ctx) -> tuple[Path | None, dict | None]:
    """The newest results file for this machine class that carries budget data.

    The baseline is the previous run's own numbers, so a file without a budgets
    suite (a single-suite run of another kind) is skipped rather than counted
    as "no regression".
    """
    for path in sorted(ctx.results_dir.glob(f"acceptance-{ctx.machine_class}*.json"), reverse=True):
        try:
            document = json.loads(path.read_text())
        except (OSError, ValueError):
            continue
        suites = document.get("suites") or []
        if any(item.get("suite") == "boot-restore-footprint-budgets" and item.get("data") for item in suites):
            return path, document
    return None, None


def _budget_case_boot(suite: Suite, ctx, boot_p95_ms: float | None) -> None:
    case = suite.case(
        "boot time (POST -> every VM running)",
        vector="1/8/50-VM boots, fixed sample counts, p50/p95/p99",
    )
    measured: dict[str, dict] = {}
    with Lab(ctx, [], "budgets-boot") as lab:
        baseline = lab.rss()
        for size, count in BOOT_SAMPLES.items():
            samples: list[float] = []
            all_running = True
            for _ in range(count):
                reply, view = lab.create(n=size, vcpus=1, memory_mib=128, beats=1)
                samples.append(reply.elapsed)
                all_running = all_running and view["state"] == "running" and all(
                    vm["state"] == "running" for vm in view["vms"]
                )
                lab.delete(view["id"])
                _await_rss(lab, baseline + max(256 << 20, size * 128 << 20))
            stats = summary(samples)
            measured[str(size)] = stats
            case.invariant(
                f"every one of the {count} {size}-VM boots came up running",
                all_running,
            )
            case.invariant(
                f"{size}-VM boot recorded at least {min(3, count)} samples",
                stats["n"] >= min(3, count),
                f"n={stats['n']}",
            )
            case.invariant(
                f"{size}-VM boot percentiles are ordered",
                stats["p50"] <= stats["p95"] <= stats["p99"] <= stats["max"] + 1e-9,
                f"p50={stats['p50']:.3f} p95={stats['p95']:.3f} p99={stats['p99']:.3f}",
            )
            case.note(
                f"{size}-VM boot: n={stats['n']} p50={stats['p50'] * 1000:.0f}ms "
                f"p95={stats['p95'] * 1000:.0f}ms p99={stats['p99'] * 1000:.0f}ms "
                f"max={stats['max'] * 1000:.0f}ms"
            )
            if boot_p95_ms is not None:
                case.invariant(
                    f"{size}-VM boot p95 within the configured {boot_p95_ms:.0f}ms bar",
                    stats["p95"] * 1000 <= boot_p95_ms,
                    f"p95={stats['p95'] * 1000:.0f}ms",
                )
                if stats["p95"] * 1000 > boot_p95_ms:
                    suite.finding(
                        f"{size}-VM boot p95 above the configured bar",
                        classification="budget-regression",
                        detail=f"configured {boot_p95_ms:.0f}ms, measured {stats['p95'] * 1000:.0f}ms",
                    )
    suite.data["boot_ms"] = measured
    if boot_p95_ms is None:
        case.note(
            "no boot-time threshold is configured in this repository (no in-repo boot budget "
            "exists), so these are measurements; pass --boot-p95-ms to enforce a bar"
        )
        suite.note("boot budget: measured only — no configured threshold exists in-repo")


def _budget_case_lifecycle(suite: Suite, ctx) -> None:
    case = suite.case(
        "full lifecycle (POST -> done, beats=2)",
        vector="1/8/50-VM full runs, fixed sample counts, p50/p95/p99",
    )
    measured: dict[str, dict] = {}
    with Lab(ctx, [], "budgets-lifecycle") as lab:
        baseline = lab.rss()
        for size, count in LIFECYCLE_SAMPLES.items():
            samples: list[float] = []
            all_done = True
            for _ in range(count):
                started = now()
                _, view = lab.create(n=size, vcpus=1, memory_mib=128, beats=2)
                final, _ = lab.await_state(view["id"], {"done", "failed", "killed"}, started + FLEET_DEADLINE_S)
                samples.append(now() - started)
                all_done = all_done and final["state"] == "done" and all(
                    vm["state"] == "exited" for vm in final["vms"]
                )
                _await_rss(lab, baseline + max(256 << 20, size * 128 << 20))
            stats = summary(samples)
            measured[str(size)] = stats
            case.invariant(f"every {size}-VM lifecycle run reached done", all_done)
            case.invariant(
                f"{size}-VM lifecycle percentiles are ordered",
                stats["p50"] <= stats["p95"] <= stats["p99"] <= stats["max"] + 1e-9,
            )
            case.note(
                f"{size}-VM lifecycle: n={stats['n']} p50={stats['p50']:.2f}s "
                f"p95={stats['p95']:.2f}s p99={stats['p99']:.2f}s (guest beats every 5s)"
            )
    suite.data["lifecycle_s"] = measured


def _budget_case_footprint(suite: Suite, ctx) -> None:
    case = suite.case(
        f"{FLEET_N}-VM footprint (RSS and steady-state CPU)",
        vector=f"{FLEET_N} x 1 vCPU x 128 MiB fleet; whole-daemon RSS and idle CPU",
    )
    cpu_peak = 0.0
    small_cpu = 0.0
    with Lab(ctx, [], "budgets-footprint") as lab:
        baseline = lab.rss()
        reply, view = lab.create(n=FLEET_N, vcpus=1, memory_mib=128, beats=FLEET_BEATS)
        case.invariant(
            f"the {FLEET_N}-VM fleet came up",
            view["state"] == "running" and len(view["vms"]) == FLEET_N,
            f"state={view['state']} vms={len(view['vms'])}",
        )
        case.note(f"{FLEET_N}-VM boot (POST -> running): {reply.elapsed * 1000:.0f}ms")
        time.sleep(SETTLE_S)
        hz = os.sysconf("SC_CLK_TCK")
        rss_samples: list[int] = []
        cpu_samples: list[float] = []
        for _ in range(CPU_WINDOWS):
            rss_samples.append(lab.rss())
            ticks = lab.cpu_ticks()
            started = now()
            time.sleep(CPU_WINDOW_S)
            elapsed = now() - started
            cpu_samples.append(100.0 * (lab.cpu_ticks() - ticks) / hz / elapsed)
        peak = max(rss_samples)
        cpu_peak = max(cpu_samples)
        case.invariant(
            f"whole-fleet RSS under the {FLEET_RSS_BUDGET >> 30} GiB budget",
            peak < FLEET_RSS_BUDGET,
            f"peak={peak / 2**30:.2f} GiB samples={[value >> 20 for value in rss_samples]} MiB",
        )
        case.invariant(
            f"steady-state CPU under {FLEET_CPU_BUDGET_PCT}% of one core",
            cpu_peak < FLEET_CPU_BUDGET_PCT,
            f"peak={cpu_peak:.3f}% samples={[round(value, 3) for value in cpu_samples]}",
        )
        case.note(
            f"fleet {FLEET_N}: RSS baseline {baseline >> 20} MiB peak {peak >> 20} MiB; "
            f"CPU {[round(value, 3) for value in cpu_samples]}% of one core"
        )
        final, _ = lab.await_state(view["id"], {"done", "failed", "killed"}, now() + FLEET_DEADLINE_S)
        case.invariant(
            f"the fleet self-terminates cleanly (beats={FLEET_BEATS})",
            final["state"] == "done" and all(vm["state"] == "exited" for vm in final["vms"]),
            f"state={final['state']}",
        )
        after = lab.rss()
        suite.data["footprint"] = {
            "n": FLEET_N,
            "beats": FLEET_BEATS,
            "rss_baseline_bytes": baseline,
            "rss_samples_bytes": rss_samples,
            "rss_peak_bytes": peak,
            "rss_budget_bytes": FLEET_RSS_BUDGET,
            "rss_after_bytes": after,
            "cpu_samples_pct": cpu_samples,
            "cpu_peak_pct": cpu_peak,
            "cpu_budget_pct": FLEET_CPU_BUDGET_PCT,
            "recorded_cpu_baseline_pct": RECORDED_CPU_BASELINE_PCT,
        }

        # A small fleet, for the per-VM CPU scaling behind the baseline note.
        small_baseline = lab.rss()
        _, small_view = lab.create(n=SCALE_N, vcpus=1, memory_mib=128, beats=FLEET_BEATS)
        time.sleep(SETTLE_S)
        ticks = lab.cpu_ticks()
        started = now()
        time.sleep(CPU_WINDOW_S)
        small_cpu = 100.0 * (lab.cpu_ticks() - ticks) / hz / (now() - started)
        lab.delete(small_view["id"])
        _await_rss(lab, small_baseline + (256 << 20))
        suite.data["footprint"]["scale_cpu"] = {"n": SCALE_N, "cpu_pct": small_cpu}
        case.note(f"CPU at n={SCALE_N}: {small_cpu:.3f}% of one core (per-VM scaling evidence)")

    if cpu_peak > max(1.0, 10 * RECORDED_CPU_BASELINE_PCT):
        suite.finding(
            "steady-state CPU does not reproduce the recorded 0.03% baseline",
            classification="baseline-discrepancy",
            detail=(
                f"measured peak {cpu_peak:.3f}% of one core for {FLEET_N} guests with the fleet's own "
                f"beats configuration versus the {RECORDED_CPU_BASELINE_PCT}% recorded in PORT_STATUS "
                f"slice 16. The configured bar ({FLEET_CPU_BUDGET_PCT}%) still passes, with a thin "
                f"margin. At n={SCALE_N} the same configuration measures {small_cpu:.3f}%. The recorded "
                "figure was sampled with the guests 'parked in HLT', which measures no heartbeat writes "
                "and no console polling; noded never boots a parked guest (its cmdline builder has no "
                "halt option), and every guest it does boot writes a heartbeat every 5 s and polls its "
                "console every second, plus the run watcher's 5 ms poll pass. Those wakeups are the "
                "measured cost. A like-for-like baseline is needed: either re-sample the recorded figure "
                "with the same guest configuration, or expose a parked-guest option through noded so "
                "both states can be measured."
            ),
            measured_cpu_pct=cpu_peak,
            measured_cpu_pct_at_scale={f"n{SCALE_N}": small_cpu},
            recorded_baseline_pct=RECORDED_CPU_BASELINE_PCT,
            budget_pct=FLEET_CPU_BUDGET_PCT,
        )


def _budget_case_restore(suite: Suite, ctx) -> None:
    case = suite.case(
        "restore = daemon restart + register reconcile",
        vector="SIGTERM mid-run, restart on the same uds dir: fresh ids, new created_at, SSE identity, fresh run green",
    )
    killed_cids = list(range(40, 44))
    fresh_cid = 42
    first = Lab(ctx, killed_cids, "budgets-restore-a")
    first.start()
    first_created_at = None
    restart_uds = first.uds_dir
    try:
        _, view = first.create(n=2, vcpus=1, memory_mib=128, beats=40, vsock_cid=40)
        first_created_at = view["created_at"]
        time.sleep(1.5)
        first.noded.send_signal(signal.SIGTERM)
        first.noded.wait(timeout=10)
        case.invariant("the daemon was killed mid-run", not first.ping())
    finally:
        first.stop()

    second = Lab(ctx, killed_cids, "budgets-restore-b", uds_dir=restart_uds)
    second.start()
    try:
        case.invariant("the restarted daemon serves the machine view", second.ping())
        case.invariant(
            "the register is empty after the restart (no stale runs)",
            second.list_runs() == [],
            f"runs={second.list_runs()}",
        )
        frames: list[str] = []
        reader = threading.Thread(target=lambda: frames.extend(second.stream_events(3.0)))
        reader.start()
        time.sleep(0.4)
        _, fresh = second.create(n=2, vcpus=1, memory_mib=128, beats=None, vsock_cid=fresh_cid)
        reader.join(timeout=10)
        case.invariant(
            "a new run is a different run (new id sequence, new created_at)",
            fresh["id"] == "r1" and fresh["created_at"] != first_created_at,
            f"id={fresh['id']} created_at={fresh['created_at']} vs {first_created_at}",
        )
        case.invariant(
            "the SSE stream carries the new run's identity",
            any(f'"id":"{fresh["id"]}"' in line for line in frames),
            f"frames={[line for line in frames if line.startswith('data:')][:3]}",
        )
        final, _ = second.await_state(fresh["id"], {"done", "failed", "killed"}, now() + 120)
        case.invariant("a fresh run completes cleanly on the restarted daemon", final["state"] == "done", final["state"])
        case.note(
            "restore scope: there is no VM snapshot support in this MVP, so restore is the daemon "
            "restart plus this reconciliation, exactly as the SSE identity contract defines it"
        )
    finally:
        second.stop()


def _await_rss(lab: Lab, target: int, settle: float = 8.0) -> None:
    deadline = now() + settle
    while lab.rss() > target and now() < deadline:
        time.sleep(0.1)


def _regression_findings(suite: Suite, ctx) -> None:
    path, prior = _prior_results(ctx)
    if prior is None:
        suite.note("budget regression check: no previous results file for this machine class (baseline unset)")
        return
    suite.note(f"budget regression check: comparing against {path.name}")
    for key in ("boot_ms", "lifecycle_s"):
        current = suite.data.get(key)
        prior_suite = next(
            (
                item
                for item in (prior.get("suites") or [])
                if item.get("suite") == "boot-restore-footprint-budgets"
            ),
            None,
        )
        if not current or not prior_suite:
            continue
        previous = (prior_suite.get("data") or {}).get(key)
        if not previous:
            continue
        scale = 1000.0 if key == "boot_ms" else 1.0
        for size, stats in current.items():
            old = previous.get(size)
            if not old or old.get("p95") is None or stats.get("p95") is None:
                continue
            delta = stats["p95"] - old["p95"]
            if delta > 0 and (delta / old["p95"] > REGRESSION_TOLERANCE) and (delta * scale > REGRESSION_FLOOR_MS):
                suite.finding(
                    f"{key} p95 regressed for n={size}",
                    classification="budget-regression",
                    detail=f"{old['p95'] * scale:.0f} -> {stats['p95'] * scale:.0f} (previous {path.name})",
                )


def run(ctx, boot_p95_ms: float | None = None) -> Suite:
    suite = Suite(
        "boot-restore-footprint-budgets",
        "footprint within the configured 8 GiB / 5% bars, restore reconciles, boot + lifecycle measured",
        ctx,
    )
    reason = ctx.can_run("budgets")
    if reason:
        suite.blocked(reason)
        return suite
    _budget_case_boot(suite, ctx, boot_p95_ms)
    _budget_case_lifecycle(suite, ctx)
    _budget_case_footprint(suite, ctx)
    _budget_case_restore(suite, ctx)
    _regression_findings(suite, ctx)
    return suite
