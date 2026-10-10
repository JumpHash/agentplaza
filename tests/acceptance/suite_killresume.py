"""Suite 3 — kill-and-resume.

The MVP has no VM snapshot/restore, so "resume" here means what the plaza
actually implements: tear-down (``DELETE``), re-spawn over the same CIDs and
UDS paths, a daemon restart, and fleet churn. Five cases, and all five must
pass:

1. ``DELETE`` tears an 8-VM run down promptly (inside the VMM's own 10 s kill
   deadline), unlinks every muxer socket, releases the guest RAM, and leaves no
   orphan process behind — with the killed run's transcript checked for torn
   records.
2. A re-spawn over the same CIDs and paths immediately afterwards succeeds.
3. A ``SIGTERM`` mid-run takes the VMs with the daemon and leaves no orphan.
4. A restarted daemon reconciles its register (fresh ids) and binds cleanly
   over the killed run's CIDs — the stale-socket path.
5. Three back-to-back generations of an 8-VM ``beats`` fleet run, each
   reaching ``done`` inside the footprint budget, with no RSS accumulation.

The kill points are drawn from the recorded seed and written into the result,
so every run is reproducible from its manifest.
"""

from __future__ import annotations

import json
import os
import random
import signal
import time
from pathlib import Path

from apz import (
    FLEET_RSS_BUDGET,
    KILL_TIMEOUT_S,
    Lab,
    Suite,
    child_processes,
    now,
    probe_unix_socket,
)

#: The CIDs each case's lab serves.
CID_DELETE = list(range(10, 18))
CID_RESTART = list(range(20, 28))
CID_CHURN = list(range(30, 38))

#: The base CID the restart case boots its reconcile run on: a CID that was
#: never live when the first daemon was killed, so the reconcile check is
#: independent of the stale-socket check.
RECONCILE_CID = 24

#: The VMs per case.
VMS_DELETE = 8
VMS_RESTART = 4
VMS_CHURN = 8

#: The seeded kill windows, in seconds after ``POST`` (the VMs are already
#: running and the guests are mid-boot/mid-session inside these).
DELETE_WINDOW = (1.2, 2.8)
SIGTERM_WINDOW = (1.0, 3.0)

#: How long the muxer sockets are given to unlink after a clean teardown.
SOCKET_SETTLE_S = 5.0

#: How long the daemon's RSS is given to fall back after a teardown.
RSS_SETTLE_S = 5.0

#: RSS may stay this far above the pre-run baseline after a teardown: KVM guest
#: pages are returned, but the allocator keeps arenas. The floor covers the
#: metadata of a large fleet; the fraction covers a partial release.
RSS_RETURN_FLOOR = 96 << 20
RSS_RETURN_FRACTION = 0.30


def _kills(ctx, window: tuple[float, float], count: int) -> list[float]:
    """``count`` kill offsets inside ``window``, drawn from the seed (recorded)."""
    generator = random.Random(f"{ctx.seed}:{window[0]}:{window[1]}")
    return [round(generator.uniform(*window), 3) for _ in range(count)]


def _stale_sockets(lab: Lab, cids: list[int], settle: float = SOCKET_SETTLE_S) -> list[str]:
    wanted = {f"vm{cid}" for cid in cids}
    deadline = now() + settle
    while True:
        stale = sorted(wanted & set(lab.uds_entries()))
        if not stale or now() > deadline:
            return stale
        time.sleep(0.05)


def _await_rss(lab: Lab, target: int) -> tuple[int, float]:
    """Wait for ``noded``'s RSS to fall to ``target``; report the settled value."""
    deadline = now() + RSS_SETTLE_S
    rss = lab.rss()
    while rss > target and now() < deadline:
        time.sleep(0.1)
        rss = lab.rss()
    return rss, now()


def _transcript_intact(serial: str) -> tuple[bool, str]:
    """Every protocol record in the capture parses; the boot banner is unique."""
    records = [line for line in serial.splitlines() if '{"type":' in line]
    broken = []
    for record in records:
        payload = record[record.index('{"type":') :]
        try:
            json.loads(payload)
        except ValueError:
            broken.append(payload[:120])
    ok = not broken and serial.count("Linux version") == 1
    return ok, f"records={len(records)} broken={broken} boot_banners={serial.count('Linux version')}"


def _case_delete(suite: Suite, ctx, offsets: list[float]) -> None:
    case = suite.case(
        "DELETE tears an 8-VM run down promptly",
        vector="DELETE mid-session: teardown time, socket unlink, RSS release, no orphan, transcript intact",
    )
    case.note(f"kill offset {offsets[0]}s after POST (seed {ctx.seed!r})")
    with Lab(ctx, CID_DELETE, "kill-delete") as lab:
        baseline = lab.rss()
        _, view = lab.create(
            n=VMS_DELETE, vcpus=1, memory_mib=128, beats=40, vsock_cid=CID_DELETE[0]
        )
        case.invariant("the fleet is up", view["state"] == "running" and len(view["vms"]) == VMS_DELETE)
        case.invariant(
            "the daemon runs its VMs in-process (no child process to orphan)",
            not _children(lab),
            f"children={_children(lab)}",
        )
        case.invariant(
            "the daemon's process group holds only itself",
            lab.process_group_members() == [lab.noded.pid],
            f"group={lab.process_group_members()} leader={lab.noded.pid}",
        )
        time.sleep(offsets[0])
        live = lab.get(view["id"])
        sessions_live = lab.vsockd_log_text().count("accepted a connection")
        case.note(f"at kill time: state={live['state']} vsock connections seen={sessions_live}")
        during = lab.rss()

        reply = lab.delete(view["id"])
        teardown = reply.elapsed
        case.invariant(
            f"teardown inside the VMM kill deadline (< {KILL_TIMEOUT_S}s)",
            teardown < KILL_TIMEOUT_S,
            f"DELETE took {teardown:.3f}s",
        )
        final = reply.json
        case.invariant("run is killed", final["state"] == "killed", final["state"])
        case.invariant(
            "every VM is killed",
            all(vm["state"] == "killed" for vm in final["vms"]),
            "states=" + ",".join(vm["state"] for vm in final["vms"]),
        )
        case.invariant(
            "no VM hit the kill timeout",
            "did not stop" not in lab.noded_log_text(),
        )
        stale = _stale_sockets(lab, CID_DELETE)
        case.invariant("every muxer UDS socket is unlinked", not stale, f"stale={stale}")
        case.invariant(
            "no service answers on the torn-down CIDs",
            all(_probe(lab.muxer_marker(cid)) != "answers" for cid in CID_DELETE),
        )
        used = max(0, during - baseline)
        allowance = max(RSS_RETURN_FLOOR, int(RSS_RETURN_FRACTION * used))
        settled, _ = _await_rss(lab, baseline + allowance)
        case.invariant(
            "guest RAM is released (RSS returns toward baseline)",
            settled <= baseline + allowance,
            f"baseline={baseline >> 20} MiB during={during >> 20} MiB "
            f"after={settled >> 20} MiB allowance={allowance >> 20} MiB",
        )
        case.note(
            f"RSS baseline {baseline >> 20} MiB, {used >> 20} MiB held by the fleet, "
            f"{settled >> 20} MiB after the teardown"
        )
        new_orphans = [pid for pid in lab.process_group_members() if pid != lab.noded.pid]
        case.invariant("no leaked VMM process in the daemon's group", not new_orphans, f"group_extra={new_orphans}")
        case.note(
            "orphan scope: this host is shared with other live daemons and a fleet, so the check is "
            "scoped to the daemon's own process group (a global pgrep would blame other tenants)"
        )
        killed_serial = next(vm.get("serial", "") for vm in final["vms"])
        intact, detail = _transcript_intact(killed_serial)
        case.invariant("the killed run's transcript has no torn records", intact, detail)
        case.invariant("the daemon is still serving after a kill", lab.ping())
        case.note(f"run {view['id']} inspected after the kill; sessions seen while live: {sessions_live}")


def _case_reuse(suite: Suite, ctx) -> None:
    case = suite.case(
        "immediate re-spawn over the same CIDs and UDS paths",
        vector="run -> DELETE -> run again on the same base CID without EADDRINUSE",
    )
    with Lab(ctx, CID_DELETE, "kill-reuse") as lab:
        _, first_view = lab.create(
            n=VMS_DELETE, vcpus=1, memory_mib=128, beats=40, vsock_cid=CID_DELETE[0]
        )
        time.sleep(1.2)
        lab.delete(first_view["id"])
        stale = _stale_sockets(lab, CID_DELETE)
        case.invariant("the first run's sockets are gone before the re-spawn", not stale, f"stale={stale}")
        mark = len(lab.noded_log_text())
        _, second = lab.create(n=VMS_DELETE, vcpus=1, memory_mib=128, beats=None, vsock_cid=CID_DELETE[0])
        final, _ = lab.await_state(second["id"], {"done", "failed", "killed"}, now() + 120)
        case.invariant("the re-spawn reached done", final["state"] == "done", final["state"])
        case.invariant(
            "every VM of the re-spawn exited",
            len(final["vms"]) == VMS_DELETE and all(vm["state"] == "exited" for vm in final["vms"]),
        )
        case.invariant("the re-spawn carried the same CIDs", [vm["guest_cid"] for vm in final["vms"]] == CID_DELETE)
        log = lab.noded_log_text()[mark:]
        case.invariant(
            "no EADDRINUSE on the reuse",
            "Address already in use" not in log,
            [line for line in log.splitlines() if "in use" in line][:2],
        )


def _case_daemon_sigterm(suite: Suite, ctx, offsets: list[float]) -> None:
    case = suite.case(
        "SIGTERM mid-run takes the VMs with the daemon",
        vector="SIGTERM the daemon mid-session: the VMs die with it, no orphan VMM process",
    )
    case.note(f"kill offset {offsets[1]}s after POST (seed {ctx.seed!r})")
    lab = Lab(ctx, CID_RESTART, "kill-sigterm")
    lab.start()
    try:
        _, view = lab.create(n=VMS_RESTART, vcpus=1, memory_mib=128, beats=40, vsock_cid=CID_RESTART[0])
        case.invariant("the fleet is up before the kill", view["state"] == "running")
        time.sleep(offsets[1])
        case.invariant("the daemon had no child process", not _children(lab), f"children={_children(lab)}")
        group = lab.process_group_members()
        case.invariant("the daemon's group holds only itself", group == [lab.noded.pid], f"group={group}")
        pid = lab.noded.pid
        lab.noded.send_signal(signal.SIGTERM)
        lab.noded.wait(timeout=10)
        case.invariant("the daemon is gone", os.path.exists(f"/proc/{pid}") is False, f"pid={pid}")
        time.sleep(0.5)
        survivors = [member for member in _group_members(pid) if member != pid]
        case.invariant("no process survives in the dead daemon's group", not survivors, f"survivors={survivors}")
        # The guest is gone with the daemon: nothing answers on its muxer path.
        verdicts = {cid: _probe(lab.muxer_marker(cid)) for cid in CID_RESTART if lab.muxer_marker(cid).exists()}
        case.invariant(
            "no live guest answers after the daemon dies",
            all(verdict != "answers" for verdict in verdicts.values()),
            f"probes={verdicts}",
        )
        case.note(f"probe verdicts on the dead daemon's muxer paths: {verdicts}")
        case.note(
            "muxer sockets left on disk after SIGTERM: "
            + ",".join(sorted(f"vm{cid}" for cid in CID_RESTART if lab.muxer_marker(cid).exists()))
        )
    finally:
        lab.stop()


def _case_restart(suite: Suite, ctx) -> None:
    case = suite.case(
        "daemon restart reconciles and binds cleanly over the killed run's CIDs",
        vector="SIGTERM mid-run, restart the daemon on the SAME uds dir, POST the same CIDs again",
    )
    first_lab = Lab(ctx, CID_RESTART, "kill-restart-a")
    first_lab.start()
    first_created_at = None
    leftovers: list[str] = []
    restart_uds = first_lab.uds_dir
    try:
        _, view = first_lab.create(n=VMS_RESTART, vcpus=1, memory_mib=128, beats=40, vsock_cid=CID_RESTART[0])
        first_created_at = view["created_at"]
        first_id = view["id"]
        case.invariant("the killed daemon's run is r1", first_id == "r1", first_id)
        time.sleep(1.2)
        first_lab.noded.send_signal(signal.SIGTERM)
        first_lab.noded.wait(timeout=10)
        leftovers = sorted(f"vm{cid}" for cid in CID_RESTART if first_lab.muxer_marker(cid).exists())
        case.note(f"leftover muxer sockets after the SIGTERM: {leftovers}")
    finally:
        first_lab.stop()

    second_lab = Lab(ctx, CID_RESTART, "kill-restart-b", uds_dir=restart_uds)
    second_lab.start()
    try:
        machine = second_lab.machine()
        case.invariant("the restarted daemon serves the API", "class" in machine, str(machine.get("class")))
        # The reconciliation contract: a new run is a *new* run even though the
        # id sequence restarts (the SSE identity rule: id + created_at).
        _, fresh = second_lab.create(n=1, vcpus=1, memory_mib=128, beats=None, vsock_cid=RECONCILE_CID)
        case.invariant(
            "a new run is a different run (fresh id sequence, new created_at)",
            fresh["id"] == "r1" and fresh["created_at"] != first_created_at,
            f"id={fresh['id']} created_at={fresh['created_at']} vs {first_created_at}",
        )
        fresh_final, _ = second_lab.await_state(fresh["id"], {"done", "failed", "killed"}, now() + 120)
        case.invariant(
            "the reconcile run completes cleanly on a fresh CID",
            fresh_final["state"] == "done",
            f"state={fresh_final['state']}",
        )

        # Now the killed daemon's CIDs: is the restart clean?
        mark = len(second_lab.noded_log_text())
        _, reused = second_lab.create(n=VMS_RESTART, vcpus=1, memory_mib=128, beats=None, vsock_cid=CID_RESTART[0])
        reused_final, _ = second_lab.await_state(reused["id"], {"done", "failed", "killed"}, now() + 120)
        binds_cleanly = reused_final["state"] == "done"
        case.invariant(
            "the restart binds cleanly over the killed run's CIDs",
            binds_cleanly,
            f"state={reused_final['state']} log=" + "; ".join(
                line for line in second_lab.noded_log_text()[mark:].splitlines() if "failed" in line or "in use" in line
            )[:400],
        )
        if not binds_cleanly:
            # Smallest condition: the *only* blocker is the leftover socket file.
            stale = sorted(f"vm{cid}" for cid in CID_RESTART if second_lab.muxer_marker(cid).exists())
            for name in stale:
                (second_lab.uds_dir / name).unlink(missing_ok=True)
            _, recovered = second_lab.create(
                n=VMS_RESTART, vcpus=1, memory_mib=128, beats=None, vsock_cid=CID_RESTART[0]
            )
            recovered_final, _ = second_lab.await_state(
                recovered["id"], {"done", "failed", "killed"}, now() + 120
            )
            case.note(
                "with the stale sockets removed by hand the same CIDs bind again "
                f"(state={recovered_final['state']}), which isolates the leftover socket file "
                "as the only blocker"
            )
            stale_errors = [
                line
                for line in second_lab.noded_log_text().splitlines()
                if "Address already in use" in line
            ]
            suite.finding(
                "a daemon restart cannot reuse the CIDs that were live when it was killed",
                classification="product-defect",
                severity="medium-high",
                affected_cell="daemon restart over a killed run's CIDs (kill-and-resume case 4)",
                detail=(
                    "SIGTERM leaves the VMM muxer's host-side Unix socket on disk "
                    "(crates/vmm/src/devices/virtio/vsock/unix/muxer.rs binds uds_path with a plain "
                    "UnixListener::bind and only unlinks it in Drop, which a signal death never runs; "
                    "the muxer's own comment documents that a SIGKILLed VM leaves one behind) and "
                    "noded installs no signal handler, so there is no clean-shutdown path that would "
                    "run those Drops. The next daemon's boot over the same CID therefore fails with "
                    "EADDRINUSE and the run is reported failed. vsockd has a stale-socket probe for "
                    "its listeners (slice 17); the muxer's path has none, so nothing ever clears it "
                    "and the CID is wedged until an operator removes the file."
                ),
                observed=f"run {reused['id']} state={reused_final['state']}; noded logs: " + " | ".join(stale_errors[:2]),
                expected="state=done (the restart binds cleanly over the killed run's CIDs)",
                reproducer=(
                    "start noded --uds-dir D --port 0; POST {n:1,...,vsock_cid:20}; SIGTERM the daemon; "
                    "start noded again on D; POST the same spec -> 201 with state=failed and "
                    "'error binding to the host-side Unix socket: Address already in use (os error 98)'; "
                    "D/vm20 exists and nothing answers on it"
                ),
                suspected_component="crates/vmm/src/devices/virtio/vsock/unix/muxer.rs (bind path) + crates/noded (no shutdown path)",
                artifacts={
                    "leftover_sockets": leftovers,
                    "noded_log": str(second_lab.noded_log),
                    "vsockd_log": str(second_lab.vsockd_log),
                },
            )
    finally:
        second_lab.stop()


def _case_churn(suite: Suite, ctx) -> None:
    case = suite.case(
        "three generations of an 8-VM beats fleet",
        vector="run -> done times 3 back-to-back on the same CIDs, each inside the footprint budget",
    )
    with Lab(ctx, CID_CHURN, "kill-churn") as lab:
        baseline = lab.rss()
        peaks: list[int] = []
        for generation in range(3):
            _, view = lab.create(n=VMS_CHURN, vcpus=1, memory_mib=128, beats=2, vsock_cid=CID_CHURN[0])
            peak = baseline
            final = None
            deadline = now() + 120
            while now() < deadline:
                final = lab.get(view["id"])
                peak = max(peak, lab.rss())
                if final["state"] in ("done", "failed", "killed"):
                    break
                time.sleep(0.2)
            peaks.append(peak)
            case.invariant(
                f"generation {generation} reached done",
                final is not None and final["state"] == "done",
                (final or {}).get("state", "never finished"),
            )
            case.invariant(
                f"generation {generation} exited every VM",
                final is not None and all(vm["state"] == "exited" for vm in final["vms"]),
                "states=" + ",".join(vm["state"] for vm in (final or {}).get("vms", [])),
            )
            case.invariant(
                f"generation {generation} stayed inside the RSS budget",
                peak < FLEET_RSS_BUDGET,
                f"peak={peak >> 20} MiB budget={FLEET_RSS_BUDGET >> 20} MiB",
            )
            stale = _stale_sockets(lab, CID_CHURN)
            case.invariant(f"generation {generation} left no muxer socket", not stale, f"stale={stale}")
            settled, _ = _await_rss(lab, baseline + max(RSS_RETURN_FLOOR, (peak - baseline) // 4))
            case.invariant(
                f"generation {generation} released its memory",
                settled < baseline + max(RSS_RETURN_FLOOR, (peak - baseline) // 4),
                f"baseline={baseline >> 20} MiB peak={peak >> 20} MiB after={settled >> 20} MiB",
            )
        growth = peaks[-1] - peaks[0]
        case.invariant(
            "no RSS accumulation across generations",
            growth <= (64 << 20),
            f"peaks MiB={[peak >> 20 for peak in peaks]} growth={growth >> 20} MiB",
        )
        case.note(f"RSS baseline {baseline >> 20} MiB; peaks {[peak >> 20 for peak in peaks]} MiB")


def _children(lab: Lab) -> list[int]:
    """``noded``'s child processes, excluding its sibling ``vsockd``."""
    children = child_processes(lab.noded.pid)
    if lab.vsockd is not None:
        children = [pid for pid in children if pid != lab.vsockd.pid]
    return children


def _group_members(leader: int) -> list[int]:
    """Every live PID whose process group is ``leader``'s."""
    members: list[int] = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text()
        except OSError:
            continue
        fields = stat.rsplit(")", 1)[-1].split()
        if len(fields) > 2 and fields[2] == str(leader):
            members.append(int(entry.name))
    return sorted(members)


def _probe(path: Path) -> str:
    return probe_unix_socket(path)


def run(ctx) -> Suite:
    suite = Suite("kill-and-resume", "5 of 5 cases", ctx)
    reason = ctx.can_run("killresume")
    if reason:
        suite.blocked(reason)
        return suite
    offsets = _kills(ctx, DELETE_WINDOW, 1) + _kills(ctx, SIGTERM_WINDOW, 2)
    suite.note(f"kill offsets from seed {ctx.seed!r}: DELETE@{offsets[0]}s SIGTERM@{offsets[1]}s")
    _case_delete(suite, ctx, offsets)
    _case_reuse(suite, ctx)
    _case_daemon_sigterm(suite, ctx, offsets)
    _case_restart(suite, ctx)
    _case_churn(suite, ctx)
    return suite
