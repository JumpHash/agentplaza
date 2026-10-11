"""Suite 6 — the seccomp jail in the boot path.

The VMM's confinement layer (``crates/vmm/src/jail``) is armed **per VMM
thread**: ``noded`` runs each VM's vCPU, vsock muxer and exit watcher as
threads of one process, and each of those calls ``Jail::arm_thread`` just
before its loop, installing ``SECCOMP_MODE_FILTER`` on itself with
``SECCOMP_RET_KILL_PROCESS`` as the default action (the shipped ``kill`` mode).
A syscall the allowlist does not name therefore kills the *whole* daemon — which
is what the incident of 2026-10-11 was: a live ``noded`` dying every ~6 minutes
because ``recvfrom`` and ``mprotect`` were missing from the list.

This suite proves the two halves of that switch in a **real boot path** — a VM
booted through the runner's own ``noded`` API, not a unit-test harness — and it
proves them from the **kernel's own view**, never from a daemon's word:

1. **Armed (the default).** While the VMs run, every ``apz_vcpu_*`` /
   ``apz_vm*_vsock`` / ``apz_vm*_exit`` thread of the daemon reads
   ``Seccomp: 2`` in ``/proc/<noded-pid>/task/<tid>/status`` — the kernel's
   per-thread record that a filter is installed. The daemon's own request loop
   reads ``Seccomp: 0``: the jail is deliberately per-thread, so one VM's
   violation cannot take the daemon's API down with it.
2. **The escape hatch.** With ``APZ_VMM_NO_JAIL=1`` in the daemon's environment
   the same threads read ``Seccomp: 0`` and the VMs still boot — asserted, not
   assumed.

The daemon under test is the suite's **own child** (``Lab`` spawns it), so the
``/proc`` scan is over a process this runner owns: it is read by pid, and a
thread that exits mid-scan is simply absent from the next read. Reading the
*per-thread* ``status`` is required rather than convenient — ``/proc/<pid>/
status`` reports only the leader thread, which is not armed.

The soak case holds a 50-VM fleet under the armed jail and asserts the daemon
is the same live process at the end and that ``journalctl -k`` recorded no
``type=1326`` refusal for it. A daemon that dies under the jail is a **finding**
(a product defect to fix, never a flake): the case fails and the kernel's audit
line is attached.
"""

from __future__ import annotations

import time

from apz import (
    FLEET_DEADLINE_S,
    SECCOMP_DISABLED,
    SECCOMP_FILTER,
    Lab,
    Suite,
    await_vmm_threads,
    kernel_seccomp_audit,
    thread_status,
    vmm_audit_attribution,
    vmm_threads,
)

#: The CIDs the two boot cases serve — disjoint from every other suite's (the
#: demo matrix uses 10..13/100..103, kill-and-resume 10..37, budgets 40, fleet
#: 50..56/100..101, treasury 130..151, social 170..178, agora 180..183).
CID_ARMED = 60
CID_OFF = 62

#: The boot cases' shape: 2 VMs x 1 vCPU, long-lived (``beats`` unset), so the
#: VMM threads stay up while the scan reads them.
BOOT_VMS = 2
#: The threads a 2-VM boot arms: 2 vCPUs + 2 vsock muxers + 2 exit watchers.
BOOT_THREADS = 3 * BOOT_VMS

#: The soak case's shape: the slice-16 footprint scale (50 x 1 vCPU x 128 MiB),
#: a guest that beats ``SOAK_BEATS`` times and reboots, so the run reaches
#: ``done`` inside the fleet deadline. ``SOAK_CID_BASE`` keeps the fleet's CIDs
#: clear of every other suite's.
SOAK_N = 50
SOAK_BEATS = 12
SOAK_CID_BASE = 110
SOAK_THREADS = 3 * SOAK_N
SOAK_SETTLE_S = 3.0


def _guarded(case, body) -> None:
    """Run a case body; a harness exception fails the case, never the suite."""
    try:
        body()
    except Exception as err:  # noqa: BLE001 — a harness fault must fail the case
        case.invariant("the case completed without a harness exception", False, f"{type(err).__name__}: {err}")


def _thread_report(threads: list[dict]) -> str:
    """The observed threads as one line of evidence (name:tid=seccomp)."""
    return " ".join(f"{thread['name']}:{thread['tid']}={thread['seccomp']}" for thread in threads)


def _vmm_kinds(threads: list[dict]) -> set[str]:
    """Which of the three armed thread kinds are present."""
    kinds = set()
    for thread in threads:
        name = thread["name"]
        if name.startswith("apz_vcpu_"):
            kinds.add("vcpu")
        elif "_vsock" in name:
            kinds.add("vsock")
        elif "_exit" in name:
            kinds.add("exit")
    return kinds


def _case_armed(suite: Suite, ctx) -> None:
    case = suite.case(
        "the jail is armed on every VMM thread of a real boot (Seccomp: 2)",
        vector=(
            "noded boots a 2-VM run through the runner's normal path; while the VMs run, every "
            "apz_vcpu_*/apz_vm*_vsock/apz_vm*_exit thread of the daemon reads Seccomp: 2 in "
            "/proc/<noded-pid>/task/<tid>/status, the request loop reads Seccomp: 0, and the kernel "
            "logged no type=1326 refusal for the daemon during the boot"
        ),
    )

    def body() -> None:
        with Lab(ctx, [CID_ARMED, CID_ARMED + 1], "jail-armed") as lab:
            pid = lab.noded.pid
            started = time.time()
            reply, view = lab.create(
                n=BOOT_VMS, vcpus=1, memory_mib=128, beats=None, vsock_cid=CID_ARMED
            )
            case.invariant("POST accepted the run (201)", reply.status == 201, f"status={reply.status}")
            case.invariant(
                "the run is running when POST answers",
                view["state"] == "running" and all(vm["state"] == "running" for vm in view["vms"]),
                f"state={view['state']}",
            )
            threads = await_vmm_threads(pid, BOOT_THREADS)
            case.task(
                f"the {BOOT_THREADS} VMM threads of the running VMs exist",
                len(threads) >= BOOT_THREADS,
                f"n={len(threads)} threads=[{_thread_report(threads)}]",
            )
            case.task(
                "all three armed thread kinds are present (vCPU, vsock, exit)",
                _vmm_kinds(threads) == {"vcpu", "vsock", "exit"},
                f"kinds={sorted(_vmm_kinds(threads))}",
            )
            armed = [thread for thread in threads if thread["seccomp"] == SECCOMP_FILTER]
            case.task(
                "every VMM thread is in seccomp filter mode (Seccomp: 2)",
                bool(threads) and len(armed) == len(threads),
                f"armed={len(armed)}/{len(threads)} threads=[{_thread_report(threads)}]",
            )
            leader = next((t for t in thread_status(pid) if t["tid"] == pid), None)
            case.invariant(
                "the daemon's request loop is not confined (the per-thread boundary)",
                leader is not None and leader["seccomp"] == SECCOMP_DISABLED,
                f"leader={leader}",
            )
            case.invariant(
                "the daemon is the same live process while the VMs run (no SIGSYS death)",
                lab.noded.poll() is None and lab.ping(),
                f"pid={pid} alive={lab.noded.poll() is None}",
            )
            audit = kernel_seccomp_audit(started)
            if not audit["available"]:
                case.note(f"kernel audit not readable here: {audit['reason']}")
            else:
                own = [line for line in audit["lines"] if vmm_audit_attribution(line)]
                case.task(
                    "the kernel logged no seccomp refusal for the daemon during the boot",
                    not own,
                    f"records={own}" if own else f"scanned {len(audit['lines'])} type=1326 record(s), none ours",
                )
                if own:
                    suite.finding(
                        "the jail refused a syscall on an armed VMM thread during a boot",
                        classification="product-defect",
                        affected_cell=f"{ctx.machine_class} jail-armed boot",
                        evidence=own,
                        suspected_component="crates/vmm/src/jail (the allowlist)",
                    )
            lab.delete(view["id"])

    _guarded(case, body)


def _case_escape_hatch(suite: Suite, ctx) -> None:
    case = suite.case(
        "the escape hatch: APZ_VMM_NO_JAIL=1 boots with the VMM threads unconfined (Seccomp: 0)",
        vector=(
            "the same 2-VM boot with APZ_VMM_NO_JAIL=1 in the daemon's own environment; every "
            "apz_* VMM thread reads Seccomp: 0 in /proc/<noded-pid>/task/<tid>/status and the VMs "
            "still boot and run"
        ),
    )

    def body() -> None:
        with Lab(ctx, [CID_OFF, CID_OFF + 1], "jail-off", env={"APZ_VMM_NO_JAIL": "1"}) as lab:
            pid = lab.noded.pid
            reply, view = lab.create(
                n=BOOT_VMS, vcpus=1, memory_mib=128, beats=None, vsock_cid=CID_OFF
            )
            case.invariant("POST accepted the run (201)", reply.status == 201, f"status={reply.status}")
            threads = await_vmm_threads(pid, BOOT_THREADS)
            case.task(
                f"the {BOOT_THREADS} VMM threads still exist (the boot path ran)",
                len(threads) >= BOOT_THREADS,
                f"n={len(threads)} threads=[{_thread_report(threads)}]",
            )
            unconfined = [thread for thread in threads if thread["seccomp"] == SECCOMP_DISABLED]
            case.task(
                "every VMM thread is unconfined (Seccomp: 0) under APZ_VMM_NO_JAIL=1",
                bool(threads) and len(unconfined) == len(threads),
                f"unconfined={len(unconfined)}/{len(threads)} threads=[{_thread_report(threads)}]",
            )
            case.invariant(
                "the VM still booted and is running (the escape hatch is a real boot, not a no-op)",
                view["state"] == "running" and lab.ping(),
                f"state={view['state']}",
            )
            lab.delete(view["id"])

    _guarded(case, body)


def _case_soak(suite: Suite, ctx) -> None:
    case = suite.case(
        f"jail soak: a {SOAK_N}-VM fleet under the armed jail — the daemon survives, the kernel logs no refusal",
        vector=(
            f"noded boots n={SOAK_N} vcpus=1 128MiB beats={SOAK_BEATS} under the armed jail; all "
            f"{SOAK_THREADS} VMM threads read Seccomp: 2 while the fleet runs, the daemon is the same "
            "live process at the end (no SIGSYS death), the fleet reaches done, and journalctl -k "
            "holds no type=1326 record for the daemon"
        ),
    )

    def body() -> None:
        with Lab(ctx, [], "jail-soak") as lab:
            pid = lab.noded.pid
            started = time.time()
            reply, view = lab.create(
                n=SOAK_N, vcpus=1, memory_mib=128, beats=SOAK_BEATS, vsock_cid=SOAK_CID_BASE
            )
            case.invariant(
                f"the {SOAK_N}-VM fleet came up (every VM running)",
                reply.status == 201
                and view["state"] == "running"
                and len(view["vms"]) == SOAK_N
                and all(vm["state"] == "running" for vm in view["vms"]),
                f"status={reply.status} state={view['state']} vms={len(view['vms'])}",
            )
            threads = await_vmm_threads(pid, SOAK_THREADS, timeout=15.0)
            armed = [thread for thread in threads if thread["seccomp"] == SECCOMP_FILTER]
            case.task(
                f"all {SOAK_THREADS} VMM threads are in seccomp filter mode (Seccomp: 2)",
                len(threads) >= SOAK_THREADS and len(armed) == len(threads),
                f"armed={len(armed)}/{len(threads)}",
            )
            time.sleep(SOAK_SETTLE_S)
            case.invariant(
                "the daemon is the same live process after the fleet settled (no SIGSYS death)",
                lab.noded.poll() is None and lab.noded.pid == pid and lab.ping(),
                f"pid={pid} alive={lab.noded.poll() is None}",
            )
            final, _ = lab.await_state(view["id"], {"done", "failed", "killed"}, time.monotonic() + FLEET_DEADLINE_S)
            case.invariant(
                "the fleet reached done under the jail",
                final["state"] == "done",
                f"state={final['state']}",
            )
            audit = kernel_seccomp_audit(started)
            if not audit["available"]:
                case.note(f"kernel audit not readable here: {audit['reason']}")
            else:
                own = [line for line in audit["lines"] if vmm_audit_attribution(line)]
                case.task(
                    "the kernel logged no seccomp refusal for the daemon during the soak",
                    not own,
                    f"records={own}" if own else f"scanned {len(audit['lines'])} type=1326 record(s), none ours",
                )
                if own:
                    suite.finding(
                        "the jail refused a syscall on an armed VMM thread during the soak",
                        classification="product-defect",
                        affected_cell=f"{ctx.machine_class} jail soak n={SOAK_N}",
                        evidence=own,
                        daemon_log=lab.noded_log_text()[-4000:],
                        suspected_component="crates/vmm/src/jail (the allowlist)",
                    )

    _guarded(case, body)


def run(ctx) -> Suite:
    suite = Suite(
        "jail-in-the-boot-path",
        "every case passes: the jail armed on every VMM thread of a real boot, the escape hatch "
        "unconfined, and a 50-VM soak with no daemon death and no kernel refusal",
        ctx,
    )
    reason = ctx.can_run("jail")
    if reason:
        suite.blocked(reason)
        return suite
    _case_armed(suite, ctx)
    _case_escape_hatch(suite, ctx)
    _case_soak(suite, ctx)
    return suite
