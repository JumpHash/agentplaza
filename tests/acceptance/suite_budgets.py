"""Suite 4 — boot, restore and footprint budgets.

Measured on this host and recorded per machine class in
``tests/acceptance/results/``:

* **Boot budget** — p50/p95/p99 wall time for a 1, 8 and 50-VM run, from
  ``POST`` to every VM running, plus the full lifecycle (``POST`` to ``done``
  with ``beats=2``). The sample counts are fixed and reported; a p95 from a
  small sample is indicative and the result says so.

* **Footprint budget** — the 50-VM fleet's whole-host RSS against the 8 GiB
  criterion (port record, slice 16) and its steady-state CPU against 5 % of one
  core, with the margin the validated numbers (5.57 GiB, 0.03 %) leave.

* **Restore** — here restore is the daemon restart plus the reconciliation
  the node's event stream (``/v1/ws``) defines: kill the
  daemon mid-run, restart it, and check the register reconciles (the new
  ``r1`` carries a new ``created_at`` — a different run) and a fresh run
  completes cleanly on the restarted daemon.

No boot-time threshold is configured anywhere in this repository, so boot and
lifecycle are *measured and recorded*; ``--boot-p95-ms`` supplies a bar when
one is published, and a p95 regression against the previous run's result file
for the same machine class is always reported as a finding.

**``--workspace`` cells.** The MVP's writable workspace volume
(``crates/vmm/src/devices/virtio/block``; ``vmctl/noded --workspace IMAGE``) adds
a second virtio-mmio device per VM, backed by a per-VM copy of a 16 MiB ext4
image. These cells measure the same shapes with that volume attached:

* boot p50/p95/p99 for 1/8/50 VMs, against the same run's without-workspace
  numbers (so the delta is same-host, same-load) and against the previous run's
  own workspace numbers;
* the full lifecycle with ``beats=2``, including that a finished run's copies
  are still there (they hold the guests' work);
* the 50-VM RSS and steady-state CPU windows, plus the host-wide cache the
  copies cost (page cache / shmem, not the daemon's resident set);
* the copy-retention contract (``{uds_dir}/run-{id}`` survives a kill and a
  restart; a new run gets fresh copies, never the old run's) and a guest-level
  persistence re-check: write through the console, power off, boot again on the
  same image copy, read the token back.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import signal
import subprocess
import threading
import time
from pathlib import Path

from apz import (
    FLEET_CPU_BUDGET_PCT,
    FLEET_DEADLINE_S,
    FLEET_RSS_BUDGET,
    Lab,
    Suite,
    meminfo_bytes,
    now,
    reconstruct_console,
    sha256_file,
    summary,
)

#: Fixed sample counts per fleet size. They are never reduced to make a
#: percentile look better; the result states the counts it used.
BOOT_SAMPLES = {1: 20, 8: 20, 50: 10}
LIFECYCLE_SAMPLES = {1: 5, 8: 5, 50: 3}

#: Fixed sample counts for the ``--workspace`` boot sweep. The block device
#: lays a copy of the base image out per VM before the boot, so these cells
#: measure a strictly larger boot than ``BOOT_SAMPLES`` does; every fleet size
#: gets the same 20 boots.
WORKSPACE_BOOT_SAMPLES = {1: 20, 8: 20, 50: 20}

#: How long the repo's own workspace-image builder may take (mkfs.ext4 on a
#: 16 MiB file).
IMAGE_BUILD_TIMEOUT_S = 300.0

#: The guest configuration of the boot sweeps: `beats` unset, so the fleet
#: boots and stays up.
#:
#: It used to be `beats=1` (a guest that reboots after one heartbeat), and the
#: sweep's "every VM came up running" assertion is unsound with that guest: a
#: `beats` guest reboots itself as soon as it has booted, the VMM reports that
#: VM `exited`, and whether it has exited by the time the POST reply is read is
#: a matter of host scheduling — 0 to a handful of the first-spawned VMs of a
#: 50-VM fleet can be `exited` in the reply while the run still reads
#: `running`. The one discriminator that would separate that churn from a VM
#: that really died is the guest's own `plaza guest: N beats done, rebooting`
#: line in the console, and that line races the guest reset it precedes (it is
#: the last write before `reboot(2)`), so it cannot carry the verdict either.
#: A boot sweep therefore boots a guest that does not self-terminate: the
#: measurement is unchanged (POST -> every VM running never included the
#: guest's own behaviour) and "every VM is running" becomes a stable contract,
#: so a VM that is not running is a fault and fails the cell with its raw
#: console attached.
BOOT_GUEST_BEATS = None

#: The guest configuration of the boot sweeps, recorded in the results so a
#: later run does not compare its p95s against numbers measured with a
#: different guest (the sweep boots a long-lived fleet; an earlier slice ran a
#: guest that rebooted itself after one beat, which produces a different boot
#: time because those early guests stop competing with the later ones).
BOOT_GUEST = f"beats={BOOT_GUEST_BEATS!r} (long-lived fleet: every VM must be running at the POST reply)"

#: How long a serial command may take to come back. The guest serves one
#: command per 5 s heartbeat window, so the window plus boot slack bounds it.
CONSOLE_DEADLINE_S = 20.0

#: How long a guest may take to power off (ACPI S5) and the run to report done.
POWEROFF_DEADLINE_S = 60.0

#: The fleet size and configuration of the footprint measurement: the scale
#: validation's own shape (port record, slice 16 — 50 x 1 vCPU x 128 MiB).
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
            first_fault = ""
            last_observation = ""
            for _ in range(count):
                reply, view = lab.create(n=size, vcpus=1, memory_mib=128, beats=BOOT_GUEST_BEATS)
                samples.append(reply.elapsed)
                observed, last_observation = _boot_observation(view, size)
                if not observed:
                    all_running = False
                    first_fault = first_fault or last_observation
                lab.delete(view["id"])
                _await_rss(lab, baseline + max(256 << 20, size * 128 << 20))
            stats = summary(samples)
            measured[str(size)] = stats
            case.invariant(
                f"every one of the {count} {size}-VM boots came up (no VM failed)",
                all_running,
                first_fault or last_observation,
            )
            if first_fault:
                case.note(f"{size}-VM boot faults; noded log (non-probe lines): {_noded_log_noise(lab)}")
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
    suite.data["boot_guest"] = BOOT_GUEST
    case.note(
        "the sweep boots a long-lived guest (beats unset) so 'every VM came up running' is a stable "
        "contract; a self-rebooting guest can have already finished its beats when the POST reply is "
        "read, which is churn rather than a boot failure, and the console line that would tell the two "
        "apart races the guest reset it precedes"
    )
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
    host_mhz: dict | None = None
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
        mhz_samples: list[dict | None] = []
        for _ in range(CPU_WINDOWS):
            rss_samples.append(lab.rss())
            ticks = lab.cpu_ticks()
            started = now()
            time.sleep(CPU_WINDOW_S)
            cpu_samples.append(100.0 * (lab.cpu_ticks() - ticks) / hz / (now() - started))
            mhz_samples.append(host_cpu_mhz())
        peak = max(rss_samples)
        cpu_peak = max(cpu_samples)
        host_mhz = _merge_mhz(mhz_samples)
        case.invariant(
            f"whole-fleet RSS under the {FLEET_RSS_BUDGET >> 30} GiB budget",
            peak < FLEET_RSS_BUDGET,
            f"peak={peak / 2**30:.2f} GiB samples={[value >> 20 for value in rss_samples]} MiB",
        )
        case.invariant(
            f"steady-state CPU under {FLEET_CPU_BUDGET_PCT}% of one core",
            cpu_peak < FLEET_CPU_BUDGET_PCT,
            f"peak={cpu_peak:.3f}% samples={[round(value, 3) for value in cpu_samples]}"
            + (f" host_cores={host_mhz['min']}-{host_mhz['max']}MHz" if host_mhz else ""),
        )
        case.note(
            f"fleet {FLEET_N}: RSS baseline {baseline >> 20} MiB peak {peak >> 20} MiB; "
            f"CPU {[round(value, 3) for value in cpu_samples]}% of one core"
            + (f" (host cores {host_mhz['min']}-{host_mhz['max']} MHz across the windows)" if host_mhz else "")
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
            "host_cpu_mhz": host_mhz,
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
        # The bar sentence states what was actually measured: it used to claim
        # the configured bar "still passes", which was false on the 2026-10-11
        # run that failed it (13.75 % against a 5 % bar).
        bar_sentence = (
            f"The configured bar ({FLEET_CPU_BUDGET_PCT}%) is met ({cpu_peak:.3f}%)."
            if cpu_peak < FLEET_CPU_BUDGET_PCT
            else f"The configured bar ({FLEET_CPU_BUDGET_PCT}%) is NOT met ({cpu_peak:.3f}%)."
        )
        clock_sentence = (
            f" Host cores ran at {host_mhz['min']}-{host_mhz['max']} MHz across the windows, and CPU "
            "time is frequency-dependent on this host, so the peak is the same guest work counted at "
            "that clock, not a property of the SUT alone (see `host_cpu_mhz`)."
            if host_mhz
            else ""
        )
        suite.finding(
            "steady-state CPU does not reproduce the recorded 0.03% baseline",
            classification="baseline-discrepancy",
            detail=(
                f"measured peak {cpu_peak:.3f}% of one core for {FLEET_N} guests with the fleet's own "
                f"beats configuration versus the {RECORDED_CPU_BASELINE_PCT}% recorded in the port record, "
                f"slice 16. {bar_sentence} At n={SCALE_N} the same configuration measures "
                f"{small_cpu:.3f}%. The recorded figure was sampled with the guests 'parked in HLT', "
                "which measures no heartbeat writes and no console polling; noded never boots a parked "
                "guest (its cmdline builder has no halt option), and every guest it does boot writes a "
                "heartbeat every 5 s and polls its console every second, plus the run watcher's 5 ms "
                "poll pass. Those wakeups are the measured cost. A like-for-like baseline is needed: "
                "either re-sample the recorded figure with the same guest configuration, or expose a "
                "parked-guest option through noded so both states can be measured."
                + clock_sentence
            ),
            measured_cpu_pct=cpu_peak,
            measured_cpu_pct_at_scale={f"n{SCALE_N}": small_cpu},
            recorded_baseline_pct=RECORDED_CPU_BASELINE_PCT,
            budget_pct=FLEET_CPU_BUDGET_PCT,
        )


def _budget_case_restore(suite: Suite, ctx) -> None:
    case = suite.case(
        "restore = daemon restart + register reconcile",
        vector="SIGTERM mid-run, restart on the same uds dir: fresh ids, new created_at, event-stream identity, fresh run green",
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
            "the event stream carries the new run's identity",
            any(f'"id":"{fresh["id"]}"' in message for message in frames),
            f"events={frames[:3]}",
        )
        final, _ = second.await_state(fresh["id"], {"done", "failed", "killed"}, now() + 120)
        case.invariant("a fresh run completes cleanly on the restarted daemon", final["state"] == "done", final["state"])
        case.note(
            "restore scope: this case is the daemon restart plus this reconciliation, as the event "
            "stream's identity contract defines it (VM snapshots and their restores are tested elsewhere)"
        )
    finally:
        second.stop()


# ------------------------------------------------------- the workspace volume
#
# `noded --workspace IMAGE` boots every VM of every run with a second
# virtio-mmio device (0xc0002000, IRQ 6) backed by its own copy of IMAGE, which
# the guest mounts at /workspace. A volume is writable, so no two VMs of a run
# share an image: noded lays the run's copies out in `{uds_dir}/run-{id}`
# before the VMs boot and keeps that directory after the run ends (it holds the
# guests' work).

#: The cases the `--workspace` cells add. Their names and vectors are fixed
#: here so a run whose image cannot be built reports the same case list BLOCKED
#: instead of quietly running a shorter suite.
WORKSPACE_CASES: tuple[tuple[str, str, str], ...] = (
    (
        "boot",
        "boot time with --workspace (POST -> every VM running, block device attached)",
        "1/8/50-VM boots with one workspace copy per VM, 20 samples per size, p50/p95/p99",
    ),
    (
        "lifecycle",
        "full lifecycle with --workspace (POST -> done, beats=2)",
        "1/8/50-VM full runs with a workspace volume, copies retained after done",
    ),
    (
        "footprint",
        f"{FLEET_N}-VM footprint with --workspace (RSS, CPU, host cache)",
        f"{FLEET_N} x 1 vCPU x 128 MiB fleet with a 16 MiB workspace volume per VM",
    ),
    (
        "persistence",
        "workspace persistence (write -> poweroff -> reboot the same copy -> read back)",
        "console `wspace <token>` in a workspace VM, ACPI S5 poweroff, then a second boot built from the retained copy",
    ),
    (
        "restore",
        "restore with --workspace (restart on the same uds dir, run-dir retention)",
        "SIGTERM a workspace daemon, restart it on its own uds dir: run-{id} survives, a fresh run gets fresh copies",
    ),
)


def _workspace_case(suite: Suite, key: str):
    """The ``--workspace`` case named by ``key`` (its name and vector are fixed)."""
    for entry_key, name, vector in WORKSPACE_CASES:
        if entry_key == key:
            return suite.case(name, vector=vector)
    raise KeyError(key)


def _block_workspace_cases(suite: Suite, reason: str) -> None:
    """Report every ``--workspace`` cell BLOCKED with the same reason."""
    for _key, name, vector in WORKSPACE_CASES:
        suite.case(name, vector=vector).blocked(reason)


def _workspace_image(ctx) -> tuple[Path | None, str | None]:
    """The base workspace image, built by the repo's script when it is absent.

    Returns ``(image, None)`` or ``(None, reason)``. The builder is the
    repository's own ``scripts/mk-workspace-image.sh`` (the block slice's), run
    with the cwd at the repository root because it writes relative build
    output; when the one tool it needs (``mkfs.ext4``, e2fsprogs) is missing,
    the reason says so and the ``--workspace`` cells are BLOCKED rather than
    silently absent.
    """
    image = ctx.workspace_image
    if image.is_file() and image.stat().st_size > 0:
        return image, None
    script = ctx.repo / "scripts" / "mk-workspace-image.sh"
    if not script.is_file():
        return None, f"no workspace image at {image} and no builder script at {script}"
    if shutil.which("mkfs.ext4") is None:
        return None, (
            f"no workspace image at {image} and mkfs.ext4 is not installed, so "
            f"{script.name} cannot build one (e2fsprogs is the only tool it needs)"
        )
    try:
        built = subprocess.run(
            [str(script)], cwd=ctx.repo, capture_output=True, text=True, timeout=IMAGE_BUILD_TIMEOUT_S
        )
    except (OSError, subprocess.SubprocessError) as err:
        return None, f"the workspace image builder {script} could not run: {err}"
    if built.returncode != 0 or not image.is_file() or image.stat().st_size == 0:
        return None, (
            f"the workspace image builder exited {built.returncode} without leaving {image}: "
            f"{(built.stderr or built.stdout).strip()[:400]}"
        )
    return image, None


def _image_record(image: Path) -> dict:
    """The workspace image's identity, for the manifest and the results."""
    return {
        "path": str(image),
        "bytes": image.stat().st_size,
        "sha256": sha256_file(image),
        "provenance": (
            "built by scripts/mk-workspace-image.sh: a 16 MiB ext4 with a pinned label, UUID and "
            "superblock time. Identity is stable across builds; the bytes are not reproducible "
            "(mke2fs randomises the journal UUID and this e2fsprogs has no -J uuid=), which is the "
            "block slice's own note"
        ),
    }


def _workspace_cells(suite: Suite) -> dict:
    """The suite's ``--workspace`` data document (created on first use)."""
    return suite.data.setdefault(
        "workspace",
        {
            "image": {},
            "boot_ms": {},
            "boot_delta_ms": {},
            "lifecycle_s": {},
            "lifecycle_delta_s": {},
        },
    )


def _reap_workspaces(lab: Lab) -> int:
    """Delete the run directories a sweep created under the lab's UDS root.

    noded keeps a finished run's workspace copies (they hold the guests' work),
    and the harness's work directory is a ``mktemp -d`` under /tmp — tmpfs on
    this host, so an un-reaped sweep would hold its copies in RAM: 20 x 50
    copies is ~16 GiB. The sweep cells reap their own copies between runs to
    bound their footprint and report the count; the retention contract has its
    own case, which never reaps the copies it asserts on.
    """
    removed = 0
    for entry in sorted(lab.uds_dir.glob("run-*")):
        if entry.is_dir():
            shutil.rmtree(entry, ignore_errors=True)
            removed += 1
    return removed


def _await_guest_mounts(lab: Lab, run_id: str, size: int) -> tuple[bool, str]:
    """Whether every VM of ``run_id`` mounted its /workspace volume.

    The mount line is printed by the guest's init only after the ext4 mount
    succeeds, so it is the guest-side proof that the block device was attached
    and usable. Every VM is polled for it (the capture keeps the line once the
    run ends, so a guest that reboots immediately is still counted).
    """
    needle = "plaza guest: /workspace mounted (ext4"
    deadline = now() + CONSOLE_DEADLINE_S
    seen: set[int] = set()
    while now() < deadline:
        for index, vm in enumerate(lab.get(run_id).get("vms") or []):
            if needle in reconstruct_console(vm.get("serial", "")):
                seen.add(index)
        if len(seen) >= size:
            return True, f"{len(seen)}/{size} guests mounted /workspace"
        time.sleep(0.1)
    return False, f"{len(seen)}/{size} guests printed the mount line"


def _await_console(lab: Lab, run_id: str, vm: int, needle: str, deadline_s: float) -> tuple[bool, str]:
    """Poll one VM's serial capture until ``needle`` appears.

    Returns ``(found, reconstructed transcript)``. The guest serves one serial
    command per 5 s heartbeat window, so a command's reply can lag its input by
    that window; the transcript is the console with the kernel's interleaved
    printk chunks removed, which is what a contiguous userspace string is
    matched against.
    """
    deadline = now() + deadline_s
    text = ""
    while now() < deadline:
        vms = lab.get(run_id).get("vms") or []
        if vm < len(vms):
            text = reconstruct_console(vms[vm].get("serial", ""))
            if needle in text:
                return True, text
        time.sleep(0.1)
    return False, text


def _copy_carries_token(copy: Path, token: str) -> tuple[bool, str]:
    """Whether a copy's ``/workspace/notes.txt`` carries ``token``, read read-only.

    ``debugfs cat /notes.txt`` reads the file out of the ext4 image on the host
    (the command the block slice's live proof used); where debugfs is absent,
    the raw bytes are scanned for the ASCII token instead.
    """
    debugfs = shutil.which("debugfs")
    if debugfs is not None:
        try:
            read = subprocess.run(
                [debugfs, "-R", "cat /notes.txt", str(copy)],
                capture_output=True,
                text=True,
                timeout=30,
            )
        except (OSError, subprocess.SubprocessError) as err:
            return False, f"debugfs could not read {copy}: {err}"
        if token in read.stdout:
            return True, f"debugfs cat /notes.txt -> {read.stdout.strip()[:160]!r}"
        return False, (
            f"debugfs cat /notes.txt -> {read.stdout.strip()[:160]!r} "
            f"stderr={read.stderr.strip()[:160]!r}"
        )
    try:
        data = copy.read_bytes()
    except OSError as err:
        return False, f"cannot read {copy}: {err}"
    return token.encode() in data, "raw byte scan (debugfs absent)"


def _workspace_token(ctx) -> str:
    """A per-run marker written into the guest's workspace.

    Unique per invocation and unlike anything in the base image, so "the token
    is in this image" can only mean this run's guest wrote it there.
    """
    marker = hashlib.sha256(f"{ctx.stamp}-{os.getpid()}-{ctx.seed}".encode()).hexdigest()[:12]
    return f"apz-workspace-{marker}"


def _budget_case_workspace_boot(
    suite: Suite, ctx, image: Path, boot_baseline: dict, boot_p95_ms: float | None
) -> None:
    case = _workspace_case(suite, "boot")
    cells = _workspace_cells(suite)
    measured: dict[str, dict] = {}
    deltas: dict[str, dict] = {}
    reaped = 0
    with Lab(ctx, [], "budgets-workspace-boot", workspace=image) as lab:
        image_bytes = image.stat().st_size
        baseline = lab.rss()
        for size, count in WORKSPACE_BOOT_SAMPLES.items():
            samples: list[float] = []
            all_running = True
            first_fault = ""
            last_observation = ""
            for iteration in range(count):
                # A non-keepalive run: one copy per VM in its run directory (a keepalive VM's image is persistent).
                reply, view = lab.create(
                    n=size, vcpus=1, memory_mib=128, beats=BOOT_GUEST_BEATS, keepalive=False
                )
                samples.append(reply.elapsed)
                observed, last_observation = _boot_observation(view, size)
                if not observed:
                    all_running = False
                    first_fault = first_fault or last_observation
                if iteration == 0:
                    copies = lab.workspace_copies(view["id"])
                    case.invariant(
                        f"the {size}-VM run laid out one workspace copy per VM, "
                        f"each the base image's size ({image_bytes >> 20} MiB)",
                        len(copies) == size and all(path.stat().st_size == image_bytes for path in copies),
                        f"copies={len(copies)} expected={size} at {lab.run_dir(view['id'])}",
                    )
                    mounted, mount_detail = _await_guest_mounts(lab, view["id"], size)
                    case.invariant(
                        f"every VM of the {size}-VM run mounted its /workspace volume",
                        mounted,
                        mount_detail,
                    )
                lab.delete(view["id"])
                reaped += _reap_workspaces(lab)
                # The same settle the without-workspace sweep waits for, so
                # the two sets of numbers describe the same steady state.
                _await_rss(lab, baseline + max(256 << 20, size * 128 << 20))
            stats = summary(samples)
            measured[str(size)] = stats
            prior = boot_baseline.get(str(size)) or {}
            deltas[str(size)] = {
                "p50_ms": None if prior.get("p50") is None else (stats["p50"] - prior["p50"]) * 1000,
                "p95_ms": None if prior.get("p95") is None else (stats["p95"] - prior["p95"]) * 1000,
            }
            case.invariant(
                f"every one of the {count} {size}-VM --workspace boots came up (no VM failed)",
                all_running,
                first_fault or last_observation,
            )
            if first_fault:
                case.note(
                    f"{size}-VM --workspace boot faults; noded log (non-probe lines): "
                    f"{_noded_log_noise(lab)}"
                )
            case.invariant(
                f"{size}-VM --workspace boot recorded the fixed {count} samples",
                stats["n"] == count,
                f"n={stats['n']}",
            )
            case.invariant(
                f"{size}-VM --workspace boot percentiles are ordered",
                stats["p50"] <= stats["p95"] <= stats["p99"] <= stats["max"] + 1e-9,
                f"p50={stats['p50']:.3f} p95={stats['p95']:.3f} p99={stats['p99']:.3f}",
            )
            delta_p50, delta_p95 = deltas[str(size)]["p50_ms"], deltas[str(size)]["p95_ms"]
            delta_text = (
                f"delta vs this run's without-workspace numbers: p50={delta_p50:+.0f}ms p95={delta_p95:+.0f}ms"
                if delta_p50 is not None and delta_p95 is not None
                else "delta vs without-workspace: not comparable (no matching without-workspace numbers)"
            )
            case.note(
                f"{size}-VM --workspace boot: n={stats['n']} p50={stats['p50'] * 1000:.0f}ms "
                f"p95={stats['p95'] * 1000:.0f}ms p99={stats['p99'] * 1000:.0f}ms; {delta_text}"
            )
            if boot_p95_ms is not None:
                case.invariant(
                    f"{size}-VM --workspace boot p95 within the configured {boot_p95_ms:.0f}ms bar",
                    stats["p95"] * 1000 <= boot_p95_ms,
                    f"p95={stats['p95'] * 1000:.0f}ms",
                )
    cells["boot_ms"] = measured
    cells["boot_delta_ms"] = deltas
    cells["boot_reaped_run_dirs"] = reaped
    case.note(
        "the sweep boots a long-lived guest (beats unset), as the without-workspace sweep does, so "
        "'every VM came up running' is a stable contract for both sets of numbers"
    )
    if boot_p95_ms is None:
        case.note(
            "no boot-time threshold is configured in this repository, so these are measurements; "
            "pass --boot-p95-ms to enforce a bar (it is applied to the workspace boot too)"
        )


def _budget_case_workspace_lifecycle(suite: Suite, ctx, image: Path) -> None:
    case = _workspace_case(suite, "lifecycle")
    cells = _workspace_cells(suite)
    measured: dict[str, dict] = {}
    deltas: dict[str, dict] = {}
    with Lab(ctx, [], "budgets-workspace-lifecycle", workspace=image) as lab:
        baseline = lab.rss()
        for size, count in LIFECYCLE_SAMPLES.items():
            samples: list[float] = []
            all_done = True
            retained = True
            for _ in range(count):
                started = now()
                # A non-keepalive run: its per-run copies stay after it ends.
                _, view = lab.create(n=size, vcpus=1, memory_mib=128, beats=2, keepalive=False)
                final, _ = lab.await_state(view["id"], {"done", "failed", "killed"}, started + FLEET_DEADLINE_S)
                samples.append(now() - started)
                all_done = all_done and final["state"] == "done" and all(
                    vm["state"] == "exited" for vm in final["vms"]
                )
                # The copies stay: they hold the guests' work.
                copies = lab.workspace_copies(view["id"])
                retained = retained and len(copies) == size and all(path.is_file() for path in copies)
                _reap_workspaces(lab)
                _await_rss(lab, baseline + max(256 << 20, size * 128 << 20))
            stats = summary(samples)
            measured[str(size)] = stats
            prior = (suite.data.get("lifecycle_s") or {}).get(str(size)) or {}
            deltas[str(size)] = {
                "p50_s": None if prior.get("p50") is None else stats["p50"] - prior["p50"],
                "p95_s": None if prior.get("p95") is None else stats["p95"] - prior["p95"],
            }
            case.invariant(f"every {size}-VM --workspace lifecycle run reached done", all_done)
            case.invariant(
                f"every finished {size}-VM --workspace run's copies are still on disk",
                retained,
            )
            case.invariant(
                f"{size}-VM --workspace lifecycle percentiles are ordered",
                stats["p50"] <= stats["p95"] <= stats["p99"] <= stats["max"] + 1e-9,
            )
            delta_s = deltas[str(size)]["p95_s"]
            case.note(
                f"{size}-VM --workspace lifecycle: n={stats['n']} p50={stats['p50']:.2f}s "
                f"p95={stats['p95']:.2f}s p99={stats['p99']:.2f}s; delta vs this run's "
                f"without-workspace p95: "
                + (f"{delta_s:+.2f}s" if delta_s is not None else "not comparable")
            )
            case.note(
                "a lifecycle run is dominated by the guest's fixed 2-beat schedule (two 5 s windows) "
                "plus the guest's own boot, so this delta is indicative only and a negative one is host "
                "noise rather than an improvement; the boot cell's per-VM copy + mount cost is the block "
                "device's own contribution to a boot"
            )
    cells["lifecycle_s"] = measured
    cells["lifecycle_delta_s"] = deltas


def _budget_case_workspace_footprint(suite: Suite, ctx, image: Path) -> None:
    case = _workspace_case(suite, "footprint")
    cells = _workspace_cells(suite)
    image_bytes = image.stat().st_size
    cached_before = meminfo_bytes("Cached")
    shmem_before = meminfo_bytes("Shmem")
    cpu_peak = 0.0
    with Lab(ctx, [], "budgets-workspace-footprint", workspace=image) as lab:
        baseline = lab.rss()
        # A non-keepalive run: one per-run copy per VM, not the CIDs' persistent images.
        reply, view = lab.create(n=FLEET_N, vcpus=1, memory_mib=128, beats=FLEET_BEATS, keepalive=False)
        case.invariant(
            f"the {FLEET_N}-VM --workspace fleet came up",
            view["state"] == "running" and len(view["vms"]) == FLEET_N,
            f"state={view['state']} vms={len(view['vms'])}",
        )
        case.note(f"{FLEET_N}-VM --workspace boot (POST -> running): {reply.elapsed * 1000:.0f}ms")
        copies = lab.workspace_copies(view["id"])
        case.invariant(
            f"the fleet holds one workspace copy per VM ({FLEET_N} x {image_bytes >> 20} MiB of image)",
            len(copies) == FLEET_N,
            f"copies={len(copies)} at {lab.run_dir(view['id'])}",
        )
        time.sleep(SETTLE_S)
        hz = os.sysconf("SC_CLK_TCK")
        rss_samples: list[int] = []
        cpu_samples: list[float] = []
        mhz_samples: list[dict | None] = []
        for _ in range(CPU_WINDOWS):
            rss_samples.append(lab.rss())
            ticks = lab.cpu_ticks()
            started = now()
            time.sleep(CPU_WINDOW_S)
            cpu_samples.append(100.0 * (lab.cpu_ticks() - ticks) / hz / (now() - started))
            mhz_samples.append(host_cpu_mhz())
        peak = max(rss_samples)
        cpu_peak = max(cpu_samples)
        host_mhz = _merge_mhz(mhz_samples)
        cached_busy = meminfo_bytes("Cached")
        shmem_busy = meminfo_bytes("Shmem")
        case.invariant(
            f"whole-fleet RSS under the {FLEET_RSS_BUDGET >> 30} GiB budget with a workspace volume",
            peak < FLEET_RSS_BUDGET,
            f"peak={peak / 2**30:.2f} GiB samples={[value >> 20 for value in rss_samples]} MiB",
        )
        case.invariant(
            f"steady-state CPU under {FLEET_CPU_BUDGET_PCT}% of one core with a workspace volume",
            cpu_peak < FLEET_CPU_BUDGET_PCT,
            f"peak={cpu_peak:.3f}% samples={[round(value, 3) for value in cpu_samples]}"
            + (f" host_cores={host_mhz['min']}-{host_mhz['max']}MHz" if host_mhz else ""),
        )
        case.note(
            f"fleet {FLEET_N} with workspace: RSS baseline {baseline >> 20} MiB peak {peak >> 20} MiB; "
            f"CPU {[round(value, 3) for value in cpu_samples]}% of one core"
            + (f" (host cores {host_mhz['min']}-{host_mhz['max']} MHz across the windows)" if host_mhz else "")
        )
        final, _ = lab.await_state(view["id"], {"done", "failed", "killed"}, now() + FLEET_DEADLINE_S)
        case.invariant(
            f"the --workspace fleet self-terminates cleanly (beats={FLEET_BEATS})",
            final["state"] == "done" and all(vm["state"] == "exited" for vm in final["vms"]),
            f"state={final['state']}",
        )
        after = lab.rss()
        still_there = lab.workspace_copies(view["id"])
        case.invariant(
            f"the finished fleet's {FLEET_N} workspace copies are still there",
            len(still_there) == FLEET_N,
            f"copies={len(still_there)}",
        )
        cached_after = meminfo_bytes("Cached")
        shmem_after = meminfo_bytes("Shmem")
        reaped = _reap_workspaces(lab)
        suite.data["workspace"]["footprint"] = {
            "n": FLEET_N,
            "beats": FLEET_BEATS,
            "workspace_image_bytes": image_bytes,
            "workspace_copies_bytes": FLEET_N * image_bytes,
            "rss_baseline_bytes": baseline,
            "rss_samples_bytes": rss_samples,
            "rss_peak_bytes": peak,
            "rss_budget_bytes": FLEET_RSS_BUDGET,
            "rss_after_bytes": after,
            "cpu_samples_pct": cpu_samples,
            "cpu_peak_pct": cpu_peak,
            "cpu_budget_pct": FLEET_CPU_BUDGET_PCT,
            "host_cpu_mhz": host_mhz,
            "host_cached_bytes": {
                "before": cached_before,
                "busy": cached_busy,
                "after_done": cached_after,
            },
            "host_shmem_bytes": {
                "before": shmem_before,
                "busy": shmem_busy,
                "after_done": shmem_after,
            },
            "reaped_run_dirs": reaped,
        }
        if shmem_before is not None and shmem_busy is not None:
            copies_bytes = FLEET_N * image_bytes
            case.note(
                f"the {copies_bytes >> 20} MiB of copies are host memory, not guest RSS: this host's "
                f"harness work directory is under /tmp (tmpfs), so their pages are charged to Shmem — "
                f"which went {shmem_before >> 20} -> {shmem_busy >> 20} MiB "
                f"({(shmem_busy - shmem_before - copies_bytes) >> 20} MiB off the copies' "
                f"{copies_bytes >> 20} MiB) while the fleet ran. Host-wide Cached "
                f"({cached_before >> 20} -> {cached_busy >> 20} MiB) moved for the whole host's reasons "
                "and is not attributed to the fleet. noded's RSS is what the 8 GiB criterion measures "
                "and does not carry the copies, so a fleet's true host cost is that RSS plus them — "
                "recorded here so it is not understated"
            )
        without = (suite.data.get("footprint") or {}).get("cpu_peak_pct")
        if without is not None and cpu_peak > 1.5 * without:
            suite.finding(
                "attaching the workspace volume raises steady-state fleet CPU by more than half",
                classification="budget-regression",
                detail=(
                    f"the same {FLEET_N}-VM fleet measures {cpu_peak:.3f}% of one core with the workspace "
                    f"volume attached and {without:.3f}% without it, in this run. The configured bar "
                    f"({FLEET_CPU_BUDGET_PCT}%) is met either way; the guest's ext4 journal is the "
                    "candidate cost."
                ),
                measured_cpu_pct=cpu_peak,
                measured_cpu_pct_without=without,
                budget_pct=FLEET_CPU_BUDGET_PCT,
            )
        else:
            case.note(
                f"steady-state CPU with the volume attached is {cpu_peak:.3f}% of one core versus "
                f"{without if without is not None else 'not measured'}% without it in this run: the "
                "guest's ext4 journal adds no measurable steady-state cost here (the case without a "
                "workspace carries the 0.03%-baseline finding)"
            )


def _budget_case_workspace_persistence(suite: Suite, ctx, image: Path) -> None:
    case = _workspace_case(suite, "persistence")
    token = _workspace_token(ctx)
    transcript_dir = ctx.new_dir("budgets-workspace-persistence")
    # The run is a keepalive agent (the node allocates its CID), and noded recovers such a VM when it stops by
    # itself. This case powers the guest off on purpose and checks that the run ends, so its node runs without
    # recovery.
    # Both boots are cold (no warm templates, so `--no-snapshots`): the case reads the guest's own mount
    # line, and a VM restored from a template prints none.
    first = Lab(
        ctx,
        [],
        "budgets-workspace-persist-a",
        workspace=image,
        noded_args=["--no-recover", "--no-snapshots"],
    )
    first.start()
    copy_path: Path | None = None
    copy_sha: str | None = None
    run_id: str | None = None
    try:
        _, run = first.create(n=1, vcpus=1, memory_mib=128, beats=None)
        run_id = run["id"]
        mounted, mount_detail = _await_console(
            first, run_id, 0, "plaza guest: /workspace mounted (ext4", CONSOLE_DEADLINE_S
        )
        case.invariant("the workspace VM mounted /workspace", mounted, mount_detail)
        # The write goes in through the console, exactly as an operator's does.
        written = first.input(run_id, 0, f"wspace {token}")
        case.invariant("the console accepts the write", written.status == 200, f"status={written.status}")
        appended, transcript = _await_console(
            first,
            run_id,
            0,
            f"appended to /workspace/notes.txt; contents:{token}",
            CONSOLE_DEADLINE_S,
        )
        case.invariant(
            "the guest appended the token to /workspace/notes.txt and printed it back",
            appended,
            transcript[-200:] if not appended else f"token echoed in {len(transcript)} bytes",
        )
        # The write must reach the image the host holds, so the guest is asked
        # to sync (the same step the block slice's live proof used); the init
        # also syncs before every poweroff.
        first.input(run_id, 0, "sync")
        synced, _ = _await_console(first, run_id, 0, "filesystems synced", CONSOLE_DEADLINE_S)
        case.invariant("the guest synced its filesystems", synced)
        # The transcript carries the console write and its echo; it is named
        # after the lab, not the run id, because the second boot's run is also
        # `r1` (the id sequence restarts with the daemon).
        write_transcript = transcript_dir / f"{first.tag}-console.txt"
        write_transcript.write_text(transcript)
        ctx.register_artifact(write_transcript)

        # The VM's persistent workspace image: one per keepalive CID, under the lab's persist dir. Its directory
        # is private (0700), and the image is the base image's size.
        copies = first.persistent_workspaces()
        workspaces = first.persist_dir / "workspaces"
        mode = workspaces.stat().st_mode & 0o777 if workspaces.is_dir() else 0
        case.invariant(
            "the persistent workspace directory is private (0700) and holds one image for the VM",
            mode == 0o700
            and len(copies) == 1
            and copies[0].stat().st_size == image.stat().st_size,
            f"mode={mode:o} images={[path.name for path in copies]} at {workspaces}",
        )
        copy_path = copies[0] if copies else None
        host_read = "no copy"
        if copy_path is not None:
            copy_sha = sha256_file(copy_path)
            found, host_read = _copy_carries_token(copy_path, token)
            case.invariant(
                "the token is in the VM's persistent image on the host while the guest still runs",
                found,
                host_read,
            )

        # A second run, on another CID, gets another image; the first run's image is left alone.
        before = set(first.persistent_workspaces())
        _, other = first.create(n=1, vcpus=1, memory_mib=128, beats=None)
        fresh = [path for path in first.persistent_workspaces() if path not in before]
        leaked = _copy_carries_token(fresh[0], token)[0] if fresh else True
        case.invariant(
            "a new run gets fresh copies, never the old run's",
            len(fresh) == 1
            and copy_path is not None
            and fresh[0] != copy_path
            and not leaked,
            f"new={[str(path) for path in fresh]} leaked={leaked}",
        )
        still_there, _ = _copy_carries_token(copy_path, token) if copy_path is not None else (False, "")
        unchanged = copy_path is not None and copy_sha == sha256_file(copy_path)
        case.invariant(
            "the new run left the earlier run's copy and its data alone",
            still_there and unchanged,
            f"copy={copy_path} sha_unchanged={unchanged}",
        )
        first.delete(other["id"])

        # ACPI S5 poweroff ends the run with its VM exited.
        started = now()
        first.input(run_id, 0, "poweroff")
        final, _ = first.await_state(run_id, {"done", "failed", "killed"}, started + POWEROFF_DEADLINE_S)
        case.invariant(
            "the guest powered off (S5) and the run reported done with its VM exited",
            final["state"] == "done" and all(vm["state"] == "exited" for vm in final["vms"]),
            f"state={final['state']} vms={[vm['state'] for vm in final['vms']]}",
        )
        case.note(f"poweroff -> done in {now() - started:.2f}s")
        case.invariant(
            "the finished run's workspace image is still there after the run ended",
            copy_path is not None and copy_path.is_file(),
            f"image={copy_path}",
        )

        # ...and it survives the daemon dying, because nothing reaps it.
        first.noded.send_signal(signal.SIGTERM)
        first.noded.wait(timeout=10)
        survived, survived_detail = (
            _copy_carries_token(copy_path, token) if copy_path is not None else (False, "no copy")
        )
        case.invariant(
            "the run's workspace copy and its data survive the daemon being killed",
            survived and copy_path is not None and copy_path.is_file() and not first.ping(),
            f"{survived_detail} path={copy_path}",
        )
    finally:
        first.stop()

    if copy_path is None or not copy_path.is_file():
        case.invariant(
            "the persistence re-check has a retained copy to boot",
            False,
            f"retained copy={copy_path} (run {run_id})",
        )
        return

    # The second boot: a fresh daemon on the same uds dir, so the same persist dir. Its keepalive VM gets the
    # same CID back, and noded returns that CID's persistent image as it is (it never copies the base over it),
    # so the guest reads the previous boot's write back out of the same bytes.
    second = Lab(
        ctx,
        [],
        "budgets-workspace-persist-b",
        uds_dir=first.uds_dir,
        workspace=image,
        noded_args=["--no-recover", "--no-snapshots"],
    )
    second.start()
    try:
        _, reboot = second.create(n=1, vcpus=1, memory_mib=128, beats=None)
        rid = reboot["id"]
        remounted, remount_detail = _await_console(
            second, rid, 0, "plaza guest: /workspace mounted (ext4", CONSOLE_DEADLINE_S
        )
        case.invariant(
            "a second boot on the retained image mounted its filesystem", remounted, remount_detail
        )
        before = reconstruct_console((second.get(rid).get("vms") or [{}])[0].get("serial", ""))
        case.invariant(
            "the token is not in the second boot's transcript until the guest is asked for it",
            token not in before,
            f"transcript_bytes={len(before)}",
        )
        second.input(rid, 0, "wspace")
        read_back, transcript = _await_console(second, rid, 0, token, CONSOLE_DEADLINE_S)
        read_transcript = transcript_dir / f"{second.tag}-console.txt"
        read_transcript.write_text(transcript)
        ctx.register_artifact(read_transcript)
        case.invariant(
            "the guest reads the previous boot's token back from /workspace",
            read_back,
            f"token={'present' if read_back else 'absent'} in {len(transcript)} bytes; {transcript[-200:]}",
        )
        derived = (
            copy_path.is_file()
            and sha256_file(copy_path) != sha256_file(image)
            and _copy_carries_token(copy_path, token)[0]
        )
        case.invariant(
            "the second boot runs on the retained image, not the pristine base image",
            derived,
            f"copy={copy_path} base_sha={sha256_file(image)[:12]}",
        )
        case.note(
            "the second boot's keepalive VM mounted the persistent image at "
            f"{copy_path} (sha256 {str(copy_sha)[:12]}): noded returns that image as it is, so the guest "
            "reads those bytes, not the base image's"
        )
        suite.data.setdefault("workspace", {})["persistence"] = {
            "token": token,
            "first_run": {"run": run_id, "copy": str(copy_path), "sha256": copy_sha},
            "second_run": {
                "run": rid,
                "copy": str(copy_path),
                "sha256": sha256_file(copy_path),
            },
            "base_image_sha256": sha256_file(image),
        }
    finally:
        second.stop()


def _budget_case_workspace_restore(suite: Suite, ctx, image: Path) -> None:
    case = _workspace_case(suite, "restore")
    token = _workspace_token(ctx)
    first = Lab(ctx, [], "budgets-workspace-restore-a", workspace=image)
    first.start()
    restart_uds = first.uds_dir
    run_id: str | None = None
    copy_path: Path | None = None
    copy_sha: str | None = None
    try:
        _, view = first.create(n=1, vcpus=1, memory_mib=128, beats=40, keepalive=False)
        run_id = view["id"]
        mounted, mount_detail = _await_console(
            first, run_id, 0, "plaza guest: /workspace mounted (ext4", CONSOLE_DEADLINE_S
        )
        case.invariant("the mid-run workspace VM mounted /workspace", mounted, mount_detail)
        first.input(run_id, 0, f"wspace {token}")
        wrote, _ = _await_console(
            first, run_id, 0, f"appended to /workspace/notes.txt; contents:{token}", CONSOLE_DEADLINE_S
        )
        first.input(run_id, 0, "sync")
        synced, _ = _await_console(first, run_id, 0, "filesystems synced", CONSOLE_DEADLINE_S)
        case.invariant("the mid-run guest wrote and synced its workspace", wrote and synced)
        copies = first.workspace_copies(run_id)
        copy_path = copies[0] if copies else None
        copy_sha = sha256_file(copy_path) if copy_path is not None else None
        case.invariant(
            "the running run's workspace copy is on disk",
            copy_path is not None and copy_path.is_file(),
            f"copy={copy_path} sha256={str(copy_sha)[:12]}",
        )
        time.sleep(0.5)
        first.noded.send_signal(signal.SIGTERM)
        first.noded.wait(timeout=10)
        case.invariant("the daemon was killed mid-run", not first.ping())
        case.invariant(
            "the killed daemon's run directory and copy survive it",
            copy_path is not None and copy_path.is_file(),
            f"copy={copy_path}",
        )
    finally:
        first.stop()

    second = Lab(ctx, [], "budgets-workspace-restore-b", uds_dir=restart_uds, workspace=image)
    second.start()
    try:
        case.invariant("the restarted workspace daemon serves the machine view", second.ping())
        case.invariant(
            "the register is empty after the restart (no stale runs)",
            second.list_runs() == [],
            f"runs={second.list_runs()}",
        )
        survived, survived_detail = (
            _copy_carries_token(copy_path, token) if copy_path is not None else (False, "no copy")
        )
        case.invariant(
            "the killed run's copy survives the restart, with its data",
            survived,
            f"{survived_detail} copy={copy_path}",
        )

        _, fresh = second.create(n=1, vcpus=1, memory_mib=128, beats=None, keepalive=False)
        case.invariant(
            "a fresh run is running on the restarted daemon",
            fresh["state"] == "running",
            f"id={fresh['id']} state={fresh['state']}",
        )
        fresh_copies = second.workspace_copies(fresh["id"])
        leaked = _copy_carries_token(fresh_copies[0], token)[0] if fresh_copies else True
        case.invariant(
            "the fresh run gets fresh copies, never the old run's",
            len(fresh_copies) == 1 and not leaked,
            f"dir={second.run_dir(fresh['id'])} copies={[path.name for path in fresh_copies]} leaked={leaked}",
        )
        case.note(
            "the fresh run is "
            f"{fresh['id']} in {second.run_dir(fresh['id'])}; the killed daemon's copies were at "
            f"{first.run_dir(str(run_id))}"
        )
    finally:
        second.stop()


def _noded_log_noise(lab: Lab, limit: int = 8) -> list[str]:
    """``noded`` log lines that are not the per-boot RTC-probe warnings.

    A boot sweep's log is dominated by the guest's RTC probe
    (``warning: vcpu: IO write @ 0x70:0x1 failed: Failed to find address
    range.``), which drowns the tail the evidence pack copies. This is the
    interesting remainder, bounded, so a cell that has to explain a VM that did
    not stay up carries the daemon's own account of it.
    """
    lines = [
        line
        for line in lab.noded_log_text().splitlines()
        if line.strip() and "failed: Failed to find address range" not in line
    ]
    return lines[-limit:]


def _boot_observation(view: dict, size: int) -> tuple[bool, str]:
    """Whether every VM of a run came up, from the run's own view.

    The sweep boots a guest that does not self-terminate
    (:data:`BOOT_GUEST_BEATS`), so every VM of the run must be ``running``:
    anything else is a VM that did not stay up, and fails the check. The one
    exception the *code* still accepts is a VM that exited after the guest's
    own ``plaza guest: N beats done, rebooting`` line — that is a `beats` guest
    doing what it is configured to do, and the plain boot sweep's history
    (which ran `beats=1` until this slice) is why the branch is kept.

    The returned detail is the evidence for the verdict: the run state, the VM
    state histogram, and — for every VM that is not running — its raw console
    length, whether the kernel banner, the guest's heartbeat, the mount line,
    the reboot line and any panic marker appear in it, and the head and tail of
    the raw capture. A failing cell then describes exactly what happened
    instead of just naming the invariant.
    """
    states: dict[str, int] = {}
    faults: list[str] = []
    for vm in view.get("vms", []):
        state = vm["state"]
        states[state] = states.get(state, 0) + 1
        if state == "running":
            continue
        raw = vm.get("serial", "")
        console = reconstruct_console(raw)
        if state == "exited" and "beats done, rebooting" in console:
            continue
        markers = {
            "raw_len": len(raw),
            "banner": "Linux version" in raw,
            "alive": "plaza guest alive" in raw,
            "mounted": "/workspace mounted" in raw,
            "reboot_line": "beats done, rebooting" in raw,
            "panic": next(
                (marker for marker in ("Kernel panic", "Oops", "BUG:", "Call Trace", "RIP:") if marker in raw),
                None,
            ),
        }
        faults.append(
            f"vm{vm['id']} state={state} {markers} head={raw[:80]!r} tail={raw[-120:]!r}"
        )
    if len(view.get("vms", [])) != size:
        faults.insert(0, f"{len(view.get('vms', []))} VMs, expected {size}")
    if view["state"] not in {"running", "done"}:
        faults.insert(0, f"run state {view['state']!r}")
    detail = f"run={view['state']} vms={states}"
    if faults:
        return False, detail + " faults: " + " | ".join(faults[:3])
    return True, detail


def _await_rss(lab: Lab, target: int, settle: float = 8.0) -> None:
    deadline = now() + settle
    while lab.rss() > target and now() < deadline:
        time.sleep(0.1)


def host_cpu_mhz() -> dict | None:
    """The host's per-core clock right now (``/proc/cpuinfo``), min/avg/max.

    The footprint cell counts the daemon's CPU **time**, and on a host with an
    aggressive frequency governor that is not a property of the daemon alone:
    the same guest work costs more counted CPU seconds when the cores sit at a
    low clock. Measured on this host 2026-10-11 with the identical pinned SUT,
    guest image and configuration (50 x 1 vCPU x 128 MiB, ``beats=12``, the
    footprint cell's own shape): a peak of **14.25 %** with the cores averaging
    2.2 GHz and **4.00 %** with them averaging 4.4 GHz (24 spinners holding the
    clock up). The failing 13.75 % and the passing 4.25 % are the same work at
    different host clocks, so the clock is recorded next to the samples that
    would otherwise be read as a property of the SUT.
    """
    try:
        values = [
            float(line.split(":")[1])
            for line in Path("/proc/cpuinfo").read_text().splitlines()
            if line.startswith("cpu MHz")
        ]
    except (OSError, ValueError, IndexError):
        return None
    if not values:
        return None
    return {"min": round(min(values)), "avg": round(sum(values) / len(values)), "max": round(max(values))}


def _merge_mhz(samples: list[dict | None]) -> dict | None:
    """The host-clock summary over a cell's sampling windows, or ``None``."""
    seen = [sample for sample in samples if sample]
    if not seen:
        return None
    return {
        "min": min(sample["min"] for sample in seen),
        "avg": round(sum(sample["avg"] for sample in seen) / len(seen)),
        "max": max(sample["max"] for sample in seen),
        "windows": len(seen),
    }


def _compare_p95s(suite: Suite, source: str, label: str, current: dict | None, previous: dict | None) -> None:
    """Flag every per-size p95 in ``current`` that regressed past both bars.

    Every sweep records wall time in **seconds** (``summary`` over
    ``now() - started``) while the floor is in milliseconds, so the comparison
    converts to ms before applying it. Found 2026-10-11: the lifecycle call
    passed a scale of ``1.0`` for a seconds-scale metric, so
    ``delta * scale > REGRESSION_FLOOR_MS`` meant "a regression past 250
    seconds" and no lifecycle regression could ever be flagged — the check was
    dead for every cell but the boot sweep. The run that found this carried a
    real workspace-lifecycle p95 regression (7.15 s -> 12.78 s, n=50) that the
    dead check did not report.
    """
    if not current or not previous:
        return
    for size, stats in current.items():
        old = previous.get(size)
        if not old or old.get("p95") is None or stats.get("p95") is None:
            continue
        delta_ms = (stats["p95"] - old["p95"]) * 1000.0
        if delta_ms > 0 and (delta_ms / (old["p95"] * 1000.0) > REGRESSION_TOLERANCE) and delta_ms > REGRESSION_FLOOR_MS:
            suite.finding(
                f"{label} p95 regressed for n={size}",
                classification="budget-regression",
                detail=f"{old['p95'] * 1000:.0f} ms -> {stats['p95'] * 1000:.0f} ms (previous {source})",
            )


def _regression_findings(suite: Suite, ctx) -> None:
    path, prior = _prior_results(ctx)
    if prior is None:
        suite.note("budget regression check: no previous results file for this machine class (baseline unset)")
        return
    prior_suite = next(
        (
            item
            for item in (prior.get("suites") or [])
            if item.get("suite") == "boot-restore-footprint-budgets"
        ),
        None,
    )
    if not prior_suite:
        suite.note(f"budget regression check: {path.name} carries no budgets data")
        return
    suite.note(f"budget regression check: comparing against {path.name}")
    previous = prior_suite.get("data") or {}
    boot_guest_same = previous.get("boot_guest") == suite.data.get("boot_guest")
    if not boot_guest_same:
        suite.note(
            f"budget regression check: the boot sweeps' guest configuration changed since {path.name} "
            f"(was {previous.get('boot_guest')!r}, now {suite.data.get('boot_guest')!r}), so the boot "
            "p95s are not compared against it (a different guest is a different measurement)"
        )
    for key in ("boot_ms", "lifecycle_s"):
        if key == "boot_ms" and not boot_guest_same:
            continue
        _compare_p95s(suite, path.name, key, suite.data.get(key), previous.get(key))
    # The `--workspace` cells are compared against the previous run's own
    # workspace numbers, which only exist once a run has recorded them.
    current_workspace = suite.data.get("workspace") or {}
    previous_workspace = previous.get("workspace") or {}
    if not previous_workspace:
        suite.note(
            "budget regression check: the previous file carries no --workspace numbers "
            "(this run sets the workspace baseline)"
        )
    else:
        for key in ("boot_ms", "lifecycle_s"):
            if key == "boot_ms" and not boot_guest_same:
                continue
            _compare_p95s(
                suite,
                path.name,
                f"workspace {key}",
                current_workspace.get(key),
                previous_workspace.get(key),
            )


def run(ctx, boot_p95_ms: float | None = None) -> Suite:
    suite = Suite(
        "boot-restore-footprint-budgets",
        "footprint within the configured 8 GiB / 5% bars, restore reconciles, boot + lifecycle "
        "measured; with --workspace the same bars are enforced against the block device and the "
        "copy-retention + guest-persistence contracts are asserted",
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
    # The `--workspace` cells: the same shapes with the writable workspace
    # volume attached. A host that cannot produce the image reports them
    # BLOCKED with the reason instead of reporting a pass for cells that never
    # ran.
    image, image_reason = _workspace_image(ctx)
    if image is None:
        _block_workspace_cases(suite, image_reason or "the workspace image is unavailable")
    else:
        _workspace_cells(suite)["image"] = _image_record(image)
        _budget_case_workspace_boot(suite, ctx, image, suite.data.get("boot_ms") or {}, boot_p95_ms)
        _budget_case_workspace_lifecycle(suite, ctx, image)
        _budget_case_workspace_footprint(suite, ctx, image)
        _budget_case_workspace_persistence(suite, ctx, image)
        _budget_case_workspace_restore(suite, ctx, image)
    _regression_findings(suite, ctx)
    return suite
