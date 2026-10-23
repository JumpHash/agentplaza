"""Suite 6 — the seccomp jail in the boot path, in both spawn modes.

**The two spawn modes of one boot path.** ``crates/vmm/src/spawner`` boots a VM
either as threads of the calling process (``SpawnMode::InProcess``, the shipped
default: what ``noded``, ``vmctl`` and ``spawn_n`` have always used) or in a
child ``apz-vmm`` process (``SpawnMode::Process``, selected with the daemon's
own ``APZ_VMM_SPAWN=process`` switch — one VM per process, which is the only
shape ``crates/vmm/src/jail::jailer`` can confine, because
``unshare(CLONE_NEWUSER)`` is refused by a multi-threaded process). This suite
asserts **both**, and the split is stated rather than implied:

* the **in-process** cases — ``_case_armed``, ``_case_escape_hatch``,
  ``_case_soak``: unchanged, and still the shipped default's coverage;
* the **per-VM process** cases — ``_case_process_mode`` (the daemon's VMs are
  jailed ``apz-vmm`` *children*: parent linkage, the jail root from
  ``/proc/<pid>/mountinfo``, ``Seccomp: 2`` per child thread, the child's
  parent-death arming, a ``/tell`` through the child-bound muxer, and the
  orphan backstop under ``SIGKILL``) and ``_case_process_api_run`` (the
  regression case for a defect this suite found: a run created through ``POST
  /v1/runs`` is killed within milliseconds in process mode — see that
  case's vector and the suite's findings).

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

import json
import os
import signal
import subprocess
import time
from pathlib import Path

from apz import (
    FLEET_DEADLINE_S,
    SECCOMP_DISABLED,
    SECCOMP_FILTER,
    Lab,
    Suite,
    await_vmm_threads,
    child_processes,
    kernel_seccomp_audit,
    thread_status,
    vmm_audit_attribution,
    vmm_threads,
)

#: The CIDs the two boot cases serve — disjoint from every other suite's (the
#: demo matrix uses 10..13/100..103, kill-and-resume 10..37, budgets 40, and the
#: soak below starts at 110).
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

# ------------------------------------------------------- the per-VM process mode
#
# `SpawnMode::Process` (`crates/vmm/src/spawner/process.rs`) boots one VM per
# child `apz-vmm`, which enters the jailer's per-VM root and arms the seccomp
# filter on its own threads. The daemon's switch is `APZ_VMM_SPAWN=process`
# (`crates/vmm/src/spawner.rs` `SPAWN_MODE_ENV`), read from the *daemon's* own
# environment — the harness sets it per lab (`Lab(env=...)`) so no other suite's
# daemon sees it.

#: The process-mode fleet's roles and first CID: a 2-role fleet booted by the
#: daemon at startup (`noded --agents`), which is the shape the live plaza runs
#: (port record, slice 77). Their CIDs are disjoint from every other suite's.
PROC_ROLES = ("builder", "reviewer")
PROC_CID_BASE = 64

#: The per-child boot log, as the child itself writes it (`crates/vmm/src/bin/
#: apz-vmm/linux.rs`): the arming line is the process's *first* statement, the
#: jailer's line names the root it entered, and the readiness line is the
#: parent's gate. Each line is the child's own words, never the daemon's.
PROC_ARMING_LINE = "apz-vmm: parent-death signal armed (SIGKILL)"

#: How long the process-mode case waits for the daemon's children to appear, for
#: a run's VMs to stop being `running`, and for the orphan backstop to take.
PROC_SETTLE_S = 20.0
PROC_ORPHAN_DEADLINE_S = 10.0
#: How long the API-created run must stay up for its case to pass (the defect it
#: pins kills the VMs within milliseconds, so any window catches it).
PROC_API_SETTLE_S = 3.0

#: The CID the API-created run case serves (disjoint from every other suite's).
PROC_API_CID = 66

#: The console-growth regression case (the `mremap` gap): a 1-VM run in process
#: mode whose console output is driven past glibc's 128 KiB mmap threshold,
#: where `realloc` calls `mremap` and the armed allowlist kills the thread that
#: owns the capture. `wspace <text>` prints the whole notes file back (bounded
#: at 4 KiB per call), so a 40-byte payload grows the serial capture
#: quadratically — ~41 B per call per line, to ~205 KiB by the 100th call.
GROW_CID = 68
GROW_CALLS = 140
GROW_PAUSE_S = 0.35
#: 7 bytes of `wspace ` plus 40: under the guest's 63-byte console read.
GROW_PAYLOAD = "m" * 40
#: The drive's goal: past the threshold with room to spare.
GROW_TARGET = 256 << 10
#: The non-vacuity floor: the capture reached the threshold's neighbourhood. A
#: run killed by the defect stops just *below* 128 KiB of captured bytes (the
#: realloc that needs `mremap` is the one growing the buffer *to* 128 KiB;
#: measured: 131,066 B), so the floor cannot be the threshold itself.
GROW_REACHED = 120 << 10


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


# ------------------------------------------------- the per-VM process's helpers


def _flag(ctx, binary: str, needle: str) -> bool:
    """Whether ``binary``'s CLI advertises ``needle`` (a ``--help`` probe).

    The suite's gating idiom: a flag the binary has not landed is gated behind
    this probe, never assumed.
    """
    try:
        out = subprocess.run(
            [str(ctx.bin(binary)), "--help"], capture_output=True, text=True, timeout=20, check=False
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return needle in out.stdout


def _sleeper(predicate, timeout: float) -> bool:
    """Poll ``predicate`` until it is true or ``timeout`` elapses."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.05)
    return predicate()


def _child_tee(ctx, tag: str) -> tuple[Path, Path]:
    """The ``apz-vmm`` wrapper the daemon's children are run through, and its log.

    **Why it exists.** The child's own boot log — the parent-death arming, the
    jailer's root, the readiness line — is captured by the daemon into
    ``VmRuntime::process_log`` and surfaced **nowhere**: it is not written to
    ``noded``'s stderr, and no endpoint carries it (the run view a client reads
    carries a VM's *serial* and no process log — both process-mode cases read
    that view, and the first records the gap as a finding). So the only way to
    read the child's own words from outside is to tap the same stream, which is
    what this wrapper does.

    It is named to the daemon through the product's own switch for exactly this
    (``APZ_VMM_CHILD``, ``crates/vmm/src/spawner.rs``), and it **``exec``s** the
    real binary, so the child is the same process with the same pid and the same
    parent as it would have without the wrapper. Each stderr line is copied to
    the harness's log prefixed with ``$$`` — the wrapper's pid, which is the
    child's pid, because ``exec`` replaces the shell in place — and then passed
    through to the daemon's own reader, so nothing the daemon relies on (the
    readiness gate) changes.
    """
    log = ctx.new_dir(f"{tag}-childlogs") / "child-stderr.log"
    log.write_text("")
    script = ctx.new_dir(f"{tag}-child") / "apz-vmm-tee.sh"
    script.write_text(
        "#!/bin/bash\n"
        f"exec \"{ctx.bin('apz-vmm')}\" \"$@\" 2> >(while IFS= read -r line; do "
        f"printf '%s %s\\n' \"$$\" \"$line\" >> \"{log}\"; printf '%s\\n' \"$line\" >&2; done)\n"
    )
    script.chmod(0o755)
    ctx.register_artifact(script)
    ctx.register_artifact(log)
    return script, log


def _proc_manifest(ctx) -> Path:
    """A 2-role manifest for ``noded --agents`` (this suite's own, not the plaza's)."""
    roles = {
        role: {
            "role": role,
            "title": f"The {role}",
            "permits": [],
            "toolsets": [],
            "budget": 1,
            "variant": None,
        }
        for role in PROC_ROLES
    }
    path = ctx.new_dir("jail-proc-manifest") / "roles.json"
    path.write_text(json.dumps({"source": "tests/acceptance/suite_jail.py", "roles": roles}, indent=2) + "\n")
    ctx.register_artifact(path)
    return path


def _child_lines(log: Path) -> dict[int, list[str]]:
    """The child boot log, grouped by the pid its wrapper stamped on each line."""
    grouped: dict[int, list[str]] = {}
    for line in (log.read_text(errors="replace") if log.exists() else "").splitlines():
        head, _, rest = line.partition(" ")
        if head.isdigit():
            grouped.setdefault(int(head), []).append(rest)
    return grouped


def _armed_threads_of(pid: int) -> list[dict]:
    """The threads a child ``apz-vmm`` arms — ``apz_vcpu_{n}``, ``apz_vm{id}_vsock``,
    ``apz_vm{id}_exit``.

    ``apz.vmm_threads``'s looser ``apz_vm`` prefix would also catch the child's
    two console threads (``apz_vmm_console``, which are *not* armed), so the
    character after the prefix must be the VM's id — a digit. The child's VM is
    always id ``0`` (``process.rs``: ``child_args`` puts no id on the command
    line), so the three kinds are ``apz_vcpu_0``, ``apz_vm0_vsock``,
    ``apz_vm0_exit``.
    """
    armed = []
    for thread in thread_status(pid):
        name = thread["name"]
        if name.startswith("apz_vcpu_") or (name.startswith("apz_vm") and len(name) > 6 and name[6].isdigit()):
            armed.append(thread)
    return armed


def _await_children(daemon_pid: int, expect: int, timeout: float) -> list[int]:
    """The daemon's own children, once there are at least ``expect`` of them.

    The parent linkage is ``/proc/<pid>/stat``'s PPid field — a pgrep-style scan
    of every live process, the same reading ``apz.child_processes`` makes (the
    other form the kernel offers is ``/proc/<daemon-pid>/task/<tid>/children``;
    this suite states the one it uses).
    """
    children = child_processes(daemon_pid)
    deadline = time.monotonic() + timeout
    while len(children) < expect and time.monotonic() < deadline:
        time.sleep(0.05)
        children = child_processes(daemon_pid)
    return children


def _jail_base(lab: Lab) -> Path:
    """Where the jailer builds the children's roots for this lab.

    ``crates/vmm/src/jail/jailer.rs`` ``default_base``: ``APZ_VMM_JAIL_ROOT``
    when the daemon names one (this suite does not), else a **sibling** of the
    run's vsock UDS directory (``{uds}.jail``). Each VM's root is a child of it,
    named ``{pid}-{vm-id}-{counter}`` by ``root_for``. The cases below do not
    take that derivation on faith: each child's mountinfo ``root`` must land
    exactly one level under this directory.
    """
    return Path(f"{lab.uds_dir}.jail")


def _child_identity(pid: int) -> str | None:
    """``/proc/<pid>/stat``'s starttime field, or ``None`` if the pid is gone.

    The identity check the orphan assertion re-reads after the daemon dies: a
    pid that was recycled would have a *different* starttime, so a reused pid is
    never mistaken for a surviving child.
    """
    try:
        after = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[-1].split()
    except OSError:
        return None
    return after[19] if len(after) > 19 else None


def _root_mount(pid: int) -> dict | None:
    """The child's ``/`` mount, as ``/proc/<pid>/mountinfo`` reports it.

    The kernel's ``mountinfo`` fields are ``id parent major:minor root
    mount-point options [optional...] - fstype source super-options``; for the
    jail this is the entry whose *mount point* is ``/``, whose ``root`` is the
    directory ``pivot_root`` moved the process's root to, and whose fstype is
    whatever holds the jail directory (a tmpfs where ``/tmp`` is one — the
    field is reported as evidence, never asserted, because that is a property of
    the host's ``/tmp``). ``root`` names a directory *within its backing
    filesystem*, so it is not a host path: the case checks its shape and settles
    the identity by inode instead.
    """
    try:
        lines = Path(f"/proc/{pid}/mountinfo").read_text(errors="replace").splitlines()
    except OSError:
        return None
    for line in lines:
        fields = line.split()
        if len(fields) < 7 or fields[4] != "/":
            continue
        separator = next((index for index, field in enumerate(fields) if index >= 6 and field == "-"), None)
        return {
            "root": fields[3],
            "mount_point": fields[4],
            "fstype": fields[separator + 1] if separator is not None and separator + 1 < len(fields) else "?",
            "line": line,
        }
    return None


def _await_children_gone(identities: dict[int, str], deadline_s: float) -> tuple[list[int], float]:
    """Wait until no recorded child pid is alive *as itself*, or the deadline passes.

    A pid counts as a survivor only when ``/proc/<pid>/stat`` still holds the
    starttime recorded before the kill, so a recycled pid is never reported as
    an orphan. Returns ``(survivors, seconds_waited)``.
    """
    started = time.monotonic()
    deadline = started + deadline_s
    survivors = _survivors(identities)
    while survivors and time.monotonic() < deadline:
        time.sleep(0.05)
        survivors = _survivors(identities)
    return survivors, time.monotonic() - started


def _survivors(identities: dict[int, str]) -> list[int]:
    return [pid for pid, identity in identities.items() if _child_identity(pid) == identity]


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
                own = [
                    line
                    for line in audit["lines"]
                    if vmm_audit_attribution(line, ctx.bin_dir)
                ]
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
                own = [
                    line
                    for line in audit["lines"]
                    if vmm_audit_attribution(line, ctx.bin_dir)
                ]
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


def _plausible_uptime(output: str) -> tuple[bool, str]:
    """Whether ``output`` is the guest's own plausible uptime.

    The ``uptime`` tool is the *guest's* (``crates/agent-harness/src/tools.rs``:
    the first two fields of the guest's ``/proc/uptime``), and the fleet under
    test was booted seconds ago — so two numbers, the first a whole VM's age
    with no more than an hour on it, is what a real answer looks like. The
    repo's own convention for this tool is ``_numeric_pair`` (two floats); the
    extra magnitude bound is what makes "plausible" mean something here.
    """
    fields = output.split()
    if len(fields) != 2:
        return False, f"expected two fields, got {fields!r}"
    try:
        up, idle = float(fields[0]), float(fields[1])
    except ValueError:
        return False, f"not numbers: {fields!r}"
    if not (0.0 <= up <= 3600.0 and idle >= 0.0):
        return False, f"implausible for a guest this fresh: up={up} idle={idle}"
    return True, f"up={up} idle={idle}"


def _case_process_mode(suite: Suite, ctx) -> None:
    case = suite.case(
        "the per-VM process mode: the daemon's VMs are jailed apz-vmm children (APZ_VMM_SPAWN=process)",
        vector=(
            "a daemon started with APZ_VMM_SPAWN=process boots its 2-role fleet (noded --agents) as two "
            "apz-vmm children; the case reads the kernel's own view of each child: the parent linkage "
            "(/proc/<pid>/stat's PPid: a child count == the VM count), /proc/<pid>/mountinfo's `/` root = "
            "that child's own jail dir under the jail base (settled by inode identity with the host-side "
            "dir) with no /etc/hostname in /proc/<pid>/root while the host has one, Seccomp: 2 on every "
            "VMM thread of the child, the child's own stderr line 'parent-death signal armed (SIGKILL)' "
            "once per child, a /tell uptime on a real fleet agent (200, two numbers — the muxer path end "
            "to end), and the backstop: no apz-vmm child alive 10 s after the daemon is SIGKILLed"
        ),
    )
    if ctx.host_os != "linux":
        case.blocked(
            "the per-VM process mode is Linux-only (apz-vmm, crates/vmm/src/bin/apz-vmm.rs); this host is "
            f"{ctx.host_os}"
        )
        return
    if not _flag(ctx, "noded", "--agents"):
        case.blocked(
            "noded --agents is not in this build's CLI (probe: noded --help); no fleet can be booted, so the "
            "process-mode case is SKIPPED"
        )
        return
    if not (ctx.bin_dir / "apz-vmm").exists():
        case.blocked(
            f"the per-VM process binary {ctx.bin_dir / 'apz-vmm'} is not built; APZ_VMM_SPAWN=process has no "
            "child to run, so the case is SKIPPED"
        )
        return

    def body() -> None:
        manifest = _proc_manifest(ctx)
        tee, child_log = _child_tee(ctx, "jail-proc")
        cids = [PROC_CID_BASE + index for index in range(len(PROC_ROLES))]
        with Lab(
            ctx,
            cids,
            "jail-proc",
            env={"APZ_VMM_SPAWN": "process", "APZ_VMM_CHILD": str(tee)},
            agents=manifest,
            agents_cid_base=PROC_CID_BASE,
            status_port=0,
            admin_port=0,
            hub_status="off",
            hub_admin="self",
        ) as lab:
            daemon = lab.noded.pid
            line = next(
                (candidate for candidate in lab.noded_log_text().splitlines() if "noded: initial role fleet" in candidate),
                "",
            )
            case.invariant(
                "the daemon booted its fleet at startup in process mode (the run under test)",
                bool(line),
                lab.noded_log_text()[-400:],
            )
            run_id = line.split("noded: initial role fleet ", 1)[1].split(":", 1)[0] if line else None
            run = lab.get(run_id) if run_id else {}
            case.invariant(
                "the run is running, one VM per role (the VM count the child count must equal)",
                run.get("state") == "running"
                and sorted(vm.get("role") for vm in run.get("vms", [])) == sorted(PROC_ROLES),
                f"state={run.get('state')} vms={[(vm.get('role'), vm.get('guest_cid')) for vm in run.get('vms', [])]}",
            )

            # (a) the daemon's children are the run's VMs, one apz-vmm each.
            children = _await_children(daemon, len(PROC_ROLES), PROC_SETTLE_S)
            comms = {
                pid: Path(f"/proc/{pid}/comm").read_text(errors="replace").strip()
                for pid in children
                if Path(f"/proc/{pid}/comm").exists()
            }
            case.task(
                f"the daemon has exactly {len(PROC_ROLES)} children, one per VM, each a live apz-vmm "
                "(a child count == the VM count; /proc/<pid>/stat's PPid)",
                len(children) == len(PROC_ROLES) and set(comms.values()) == {"apz-vmm"},
                f"children={children} comms={comms}",
            )

            # (b) each child's own root is a jail dir, and it is not the host's.
            base = _jail_base(lab)
            host_hostname = Path("/etc/hostname").exists()
            for pid in children:
                root_mount = _root_mount(pid)
                # `mountinfo`'s `root` is the directory *within the backing
                # filesystem* that forms the mount's root, so it is not a host
                # path (here /tmp is itself a tmpfs: the field reads without its
                # `/tmp` prefix). Its shape is what is checkable — the child's
                # own pid prefixes the directory, and that directory is a child
                # of the jail base — and the identity is settled by comparing
                # the inode /proc/<pid>/root resolves to against the host-side
                # jail dir's, which no path spelling can fake.
                jail = Path("/") / root_mount["root"] if root_mount else None
                name = jail.name if jail else ""
                host_dir = base / name if name else None
                same_inode = False
                child_identity_stat = "?"
                host_stat_text = "?"
                if host_dir is not None and host_dir.is_dir():
                    try:
                        child_root = Path(f"/proc/{pid}/root").stat()
                        host_stat = host_dir.stat()
                        same_inode = (child_root.st_dev, child_root.st_ino) == (host_stat.st_dev, host_stat.st_ino)
                        child_identity_stat = f"({child_root.st_dev},{child_root.st_ino})"
                        host_stat_text = f"({host_stat.st_dev},{host_stat.st_ino})"
                    except OSError as err:
                        child_identity_stat = f"unreadable: {err}"
                case.task(
                    f"child {pid}: its `/` mount's root {name or '?'} is its own jail dir under the jail base "
                    "(/proc/<pid>/mountinfo root=, the directory pivot_root moved the process's root to)",
                    jail is not None
                    and name.split("-", 1)[0] == str(pid)
                    and jail.parent.name == base.name
                    and host_dir is not None
                    and host_dir.is_dir(),
                    f"mount={root_mount} base={base} host_dir={host_dir}",
                )
                case.task(
                    f"child {pid}: /proc/<pid>/root *is* that jail dir (same inode as {base.name}/{name})",
                    same_inode,
                    f"child_root={child_identity_stat} host={host_stat_text} host_dir={host_dir}",
                )
                root_has_hostname = Path(f"/proc/{pid}/root/etc/hostname").exists()
                case.task(
                    f"child {pid}: /proc/<pid>/root holds no /etc/hostname while the host's does "
                    "(the pivot really moved the child's root)",
                    host_hostname and not root_has_hostname,
                    f"host /etc/hostname={host_hostname} child /proc/{pid}/root/etc/hostname={root_has_hostname}",
                )
            names = sorted(entry.name for entry in base.iterdir()) if base.is_dir() else []
            case.task(
                "the jail base holds one root per child, each named for the child's own pid "
                "(the jail dirs' pid prefixes == the children)",
                sorted(int(name.split("-", 1)[0]) for name in names if name.split("-", 1)[0].isdigit())
                == sorted(children),
                f"jail_base={base} entries={names} children={children}",
            )

            # (c) every VMM thread of every child is confined.
            for pid in children:
                threads = _armed_threads_of(pid)
                armed = [thread for thread in threads if thread["seccomp"] == SECCOMP_FILTER]
                case.task(
                    f"child {pid}: every VMM thread is in seccomp filter mode (Seccomp: 2)",
                    bool(threads) and len(armed) == len(threads),
                    f"armed={len(armed)}/{len(threads)} threads=[{_thread_report(threads)}]",
                )
                case.task(
                    f"child {pid}: all three armed thread kinds are present (vCPU, vsock, exit)",
                    _vmm_kinds(threads) == {"vcpu", "vsock", "exit"},
                    f"kinds={sorted(_vmm_kinds(threads))}",
                )
                leader = next((thread for thread in thread_status(pid) if thread["tid"] == pid), None)
                case.invariant(
                    f"child {pid}: the child's own boot thread is not confined (the per-thread boundary)",
                    leader is not None and leader["seccomp"] == SECCOMP_DISABLED,
                    f"leader={leader}",
                )

            # (d) the parent-death arming, per child, on the stream the daemon
            # reads: the readiness gate *is* a line on that stream, so a boot
            # that completed is proof the daemon reads the child's stderr.
            grouped = _child_lines(child_log)
            arming = {pid: lines.count(PROC_ARMING_LINE) for pid, lines in grouped.items()}
            # The tee wraps every apz-vmm the daemon starts, so the log can also carry apz-vmm children
            # that are not the run's VMs (observed: warm-template builders, started before the run's VMs).
            # Each is a jailed child too: every child on the log must be armed exactly once, and every run
            # child must be among them.
            case.task(
                "the parent-death arming is observable once per child, on the child's own stderr "
                "(the stream the daemon's readiness gate reads)",
                all(arming.get(pid) == 1 for pid in children) and all(count == 1 for count in arming.values()),
                f"arming_lines={arming} children={children} log={child_log}",
            )
            extra = sorted(set(arming) - set(children))
            if extra:
                case.note(
                    f"the child log also carries {len(extra)} apz-vmm child(ren) that are not the run's VMs "
                    f"({extra}); each is armed once as well"
                )
            case.note(
                "the child's log is read here through the product's own APZ_VMM_CHILD switch (with an exec "
                "wrapper that tees the child's stderr), because the daemon captures that stream into "
                "VmRuntime::process_log and surfaces it nowhere: noded.log carries no child line, and "
                "GET /v1/runs/{id} returns a VM's serial, not its process log. Recorded as a finding."
            )
            if PROC_ARMING_LINE not in lab.noded_log_text():
                suite.finding(
                    "the per-VM process's own boot log — the jailer's root, the parent-death arming — is "
                    "captured in memory and surfaced nowhere",
                    classification="product-gap",
                    severity="low",
                    affected_cell=f"{ctx.machine_class} process mode",
                    observed=(
                        "the children's stderr carries the jailer's line and the arming line; noded.log carries "
                        "neither, and no endpoint exposes VmRuntime::process_log (a run's VM rows carry serial only)"
                    ),
                    expected="an operator can read the child's boot log for a VM (a log line or an API field)",
                    evidence=[str(child_log), str(lab.noded_log)],
                    suspected_component=(
                        "crates/noded (no log line for VmRuntime::process_log) + crates/vmm/src/spawner "
                        "(VmHandle::process_log has no API)"
                    ),
                )

            # (e) a real session through the child-bound muxer.
            connected = _sleeper(
                lambda: any(row.get("cid") == PROC_CID_BASE for row in lab.status().get("agents", [])),
                PROC_SETTLE_S,
            )
            # The hub's own tell (`POST /v1/agents/{cid}/tell`): the reply is `{ok, output}`, and the CID is the route's.
            tell = lab.admin_tell(PROC_CID_BASE, "uptime")
            output = str(tell.json.get("output", "")) if tell.status == 200 else ""
            plausible, why = _plausible_uptime(output)
            case.invariant(
                "a /tell uptime on a real agent of the fleet answers 200 with a plausible uptime "
                "(the muxer the child bound, end to end)",
                tell.status == 200
                and tell.json.get("ok") is True
                and plausible,
                f"agent_connected={connected} {tell.status} {tell.body[:200]} ({why})",
            )

            # (f) the orphan backstop: SIGKILL the daemon; its children must go.
            identities = {pid: _child_identity(pid) for pid in children}
            case.invariant(
                "every child is live with a readable identity before the kill (the orphan assertion cannot be vacuous)",
                bool(identities) and all(identity is not None for identity in identities.values()),
                f"identities={identities}",
            )
            os.kill(daemon, signal.SIGKILL)
            lab.noded.wait(timeout=10)
            case.invariant(
                "the daemon is dead (SIGKILL)",
                lab.noded.poll() is not None,
                f"returncode={lab.noded.poll()}",
            )
            survivors, waited = _await_children_gone(identities, PROC_ORPHAN_DEADLINE_S)
            case.task(
                f"no apz-vmm child of the killed daemon is alive within {PROC_ORPHAN_DEADLINE_S:.0f} s "
                "(the PR_SET_PDEATHSIG(SIGKILL) backstop)",
                not survivors,
                f"survivors={survivors} waited={waited:.2f}s of {PROC_ORPHAN_DEADLINE_S:.0f}s "
                f"(pids={sorted(identities)}; identity re-checked by /proc/<pid>/stat's starttime)",
            )

    _guarded(case, body)


def _case_process_api_run(suite: Suite, ctx) -> None:
    case = suite.case(
        "an API-created run in process mode: the VMs a POST booted are still up after the request",
        vector=(
            "a daemon started with APZ_VMM_SPAWN=process boots a 2-VM run through POST /v1/runs; after a "
            f"{PROC_API_SETTLE_S:.0f} s settle the run is still running, the daemon still has its 2 apz-vmm "
            "children, and the guests' console captures are non-empty — the request that created the VMs must "
            "not be able to take them down. FAILS on HEAD: apz-vmm arms PR_SET_PDEATHSIG(SIGKILL) against the "
            "thread that spawned it (the HTTP request thread, which exits when the response is written)"
        ),
    )
    if ctx.host_os != "linux":
        case.blocked(
            f"the per-VM process mode is Linux-only (apz-vmm); this host is {ctx.host_os}"
        )
        return
    if not (ctx.bin_dir / "apz-vmm").exists():
        case.blocked(f"the per-VM process binary {ctx.bin_dir / 'apz-vmm'} is not built; the case is SKIPPED")
        return

    def body() -> None:
        tee, child_log = _child_tee(ctx, "jail-proc-api")
        cids = [PROC_API_CID, PROC_API_CID + 1]
        with Lab(
            ctx,
            cids,
            "jail-proc-api",
            env={"APZ_VMM_SPAWN": "process", "APZ_VMM_CHILD": str(tee)},
        ) as lab:
            daemon = lab.noded.pid
            reply, view = lab.create(n=2, vcpus=1, memory_mib=128, beats=None, vsock_cid=PROC_API_CID)
            case.invariant("POST accepted the run (201)", reply.status == 201, f"status={reply.status}")
            case.invariant(
                "the run is running when POST answers",
                view["state"] == "running" and all(vm["state"] == "running" for vm in view["vms"]),
                f"state={view['state']}",
            )
            time.sleep(PROC_API_SETTLE_S)
            final = lab.get(view["id"])
            states = [(vm["state"], len(vm.get("serial") or "")) for vm in final["vms"]]
            children = child_processes(daemon)
            case.task(
                f"the run is still running {PROC_API_SETTLE_S:.0f} s after the POST (every VM running)",
                final["state"] == "running" and all(vm["state"] == "running" for vm in final["vms"]),
                f"state={final['state']} vms={states}",
            )
            case.task(
                "the daemon still has its 2 apz-vmm children (the VMs outlived their own boot)",
                len(children) == 2,
                f"children={children}",
            )
            case.task(
                "the VMs' console captures are non-empty (the children streamed their guests' serial)",
                all(length > 0 for _, length in states),
                f"vms={states}",
            )
            if final["state"] != "running" or len(children) != 2:
                grouped = _child_lines(child_log)
                armed = [pid for pid, lines in grouped.items() if PROC_ARMING_LINE in lines]
                suite.finding(
                    "a run created through POST /v1/runs is killed in the per-VM process mode: the child's "
                    "PR_SET_PDEATHSIG(SIGKILL) fires when the thread that spawned it exits",
                    classification="product-defect",
                    severity="high",
                    affected_cell=f"{ctx.machine_class} process mode, API-created runs",
                    observed=(
                        f"the POST answered 201 running and the children booted "
                        f"({len(armed)} child log(s) carry the arming line), then the run ended "
                        f"{final['state']} with the VMs {[state for state, _ in states]} and "
                        f"{len(children)} child process(es) left"
                    ),
                    expected="the run stays running for as long as a client keeps it; the VMs are children of the daemon",
                    mechanism=(
                        "apz-vmm arms the parent-death signal as its first statement "
                        "(crates/vmm/src/bin/apz-vmm/linux.rs) and the kernel delivers it when the *thread "
                        "that created the child* exits (prctl(2): 'the parent ... is the thread that created "
                        "this process'). noded boots a run's VMs on the HTTP request thread, which exits when "
                        "the response is written — so the VM is SIGKILLed milliseconds after the run reaches "
                        "running. Measured in isolation: a child spawned from a thread that exits after the "
                        "child's prctl dies by signal 9 with its parent process alive; spawned from a "
                        "long-lived thread (the daemon's main, which is how noded --agents boots its fleet) it "
                        "survives. This is why the live plaza's 21-VM fleet is unaffected."
                    ),
                    evidence=[str(child_log), str(lab.noded_log)],
                    suspected_component=(
                        "crates/noded (the run-creation path spawns its VMs on the request thread) and "
                        "crates/vmm/src/bin/apz-vmm/linux.rs (arming against the creating thread)"
                    ),
                    reproducer=(
                        "scripts/acceptance.sh jail  ->  the case 'an API-created run in process mode ...'; "
                        "or by hand: start noded with APZ_VMM_SPAWN=process, POST /v1/runs n=2, and read "
                        "the run 3 s later (state done/killed, no apz-vmm children)"
                    ),
                )

    _guarded(case, body)


def _case_grown_console_buffer(suite: Suite, ctx) -> None:
    """The jail must admit the allocator: a grown console capture is not `mremap`-free.

    New failure mode, 2026-10-11 — surfaced by this run's own kernel soak and
    confirmed against a pinned HEAD build; added to the fixed suite as its
    regression case. ``crates/vmm/src/jail``'s armed allowlist excludes
    ``mremap`` on the stated ground that "the guest-memory regions are mapped
    once; nothing remaps", and its derivation trace (``TRACE_OBSERVED``) never
    saw it either, because the traced fleet was short-lived and none of its
    buffers crossed the threshold. The VMM's serial capture is a growing ``Vec``
    (``MAX_SERIAL_CAPTURE``, 4 MiB), and glibc's ``realloc`` issues ``mremap``
    for any chunk past its mmap threshold — 128 KiB by default — so a guest
    whose console output crosses that threshold kills the VMM thread that owns
    the capture under the shipped ``SECCOMP_RET_KILL_PROCESS``. In per-VM
    process mode that is the VM's ``apz-vmm`` child; in the shipped in-process
    mode the same thread is a thread of ``noded`` itself.

    Measured on the pinned build (``pin/apz-vmm``, process mode) before this case
    existed: the run's VM went ``killed`` with its capture at 131,066 B, and the
    kernel logged ``type=1326 comm="apz_vcpu_0" sig=31 syscall=25
    code=0x80000000`` — syscall 25 is ``mremap`` and ``code=0x80000000`` is
    ``SECCOMP_RET_KILL_PROCESS``. The in-process mode did *not* reproduce it in
    the same experiment (its allocator stayed on the heap past 368 KiB of
    capture), which is why the case drives the mode where the threshold is
    deterministic: a fresh ``apz-vmm`` child starts with glibc's initial 128 KiB
    threshold.
    """
    case = suite.case(
        "the jail admits the allocator: a VMM thread survives a guest console that grows past the mmap "
        "threshold (mremap)",
        vector=(
            f"process mode; a 1-VM run is booted through POST and its console driven with `wspace <text>` "
            f"(the guest prints the whole notes file back, so the serial capture grows ~41 B/call) until the "
            f"capture passes {GROW_TARGET >> 10} KiB, past glibc's 128 KiB mmap threshold where "
            "realloc calls mremap; the VM must stay running and the kernel must log no type=1326 refusal "
            "for this lab's own VMM processes"
        ),
    )
    if ctx.host_os != "linux":
        case.blocked(f"the per-VM process mode is Linux-only (apz-vmm); this host is {ctx.host_os}")
        return
    if not (ctx.bin_dir / "apz-vmm").exists():
        case.blocked(
            f"the per-VM process binary {ctx.bin_dir / 'apz-vmm'} is not built; the case is SKIPPED"
        )
        return
    if not ctx.workspace_image.is_file():
        case.blocked(
            f"the workspace base image {ctx.workspace_image} is absent; the guest's `wspace` amplifier "
            "needs a mounted volume, so the case is SKIPPED"
        )
        return

    def body() -> None:
        started = time.time()
        with Lab(
            ctx,
            [GROW_CID],
            "jail-grow",
            env={"APZ_VMM_SPAWN": "process"},
            workspace=ctx.workspace_image,
            status_port=0,
            admin_port=0,
            hub_status="off",
            hub_admin="self",
        ) as lab:
            daemon = lab.noded.pid
            _, view = lab.create(n=1, vcpus=1, memory_mib=128, beats=None, vsock_cid=GROW_CID)
            run_id = view["id"]
            children = _await_children(daemon, 1, PROC_SETTLE_S)
            case.invariant(
                "the run's VM is a live apz-vmm child before the drive (the case cannot be vacuous)",
                len(children) == 1,
                f"children={children} run={view.get('state')}",
            )
            calls = 0
            capture = 0
            state = "running"
            for calls in range(1, GROW_CALLS + 1):
                lab.input(run_id, 0, f"wspace {GROW_PAYLOAD}")
                time.sleep(GROW_PAUSE_S)
                try:
                    current = lab.get(run_id)
                except Exception as err:  # the daemon itself is gone
                    state = f"api refused ({type(err).__name__})"
                    break
                vms = current.get("vms") or [{}]
                capture = len(vms[0].get("serial", ""))
                state = vms[0].get("state", "?")
                if capture >= GROW_TARGET or state != "running":
                    break
            case.invariant(
                f"the guest's console capture reached the mmap threshold's neighbourhood ({GROW_REACHED >> 10} "
                "KiB) — the case is not vacuous",
                capture >= GROW_REACHED,
                f"capture={capture} B after {calls} call(s)",
            )
            case.task(
                "the run's VM is still running after its capture crossed glibc's 128 KiB mmap threshold "
                "(its VMM thread survived the allocator)",
                state == "running",
                f"state={state} capture={capture} B calls={calls}",
            )
            pids = set(children) | {daemon}
            audit = kernel_seccomp_audit(started)
            if not audit["available"]:
                case.note(f"kernel audit not readable here: {audit['reason']}")
            else:
                own = [
                    line
                    for line in audit["lines"]
                    if vmm_audit_attribution(line, ctx.bin_dir)
                    and any(f"pid={pid} " in line for pid in pids)
                ]
                case.task(
                    "the kernel logged no seccomp refusal for this lab's own VMM processes",
                    not own,
                    f"records={own}" if own else f"scanned {len(audit['lines'])} type=1326 record(s), none ours",
                )
            if state != "running":
                suite.finding(
                    "a VMM thread is killed by the jail's own allowlist when a growable buffer crosses "
                    "glibc's mmap threshold: `mremap` is excluded but the serial capture remaps",
                    classification="product-defect",
                    severity="high",
                    affected_cell=f"{ctx.machine_class} process mode, a guest with a long console capture",
                    observed=(
                        f"the run's VM left `running` ({state}) with its serial capture at {capture} B "
                        f"after {calls} console calls; the kernel recorded "
                        "comm=\"apz_vcpu_0\" sig=31 syscall=25 (mremap) code=0x80000000 "
                        "(SECCOMP_RET_KILL_PROCESS) for one of this lab's own apz-vmm children"
                    ),
                    expected=(
                        "a jailed VMM thread survives its own allocator: the guest's console may grow to "
                        "MAX_SERIAL_CAPTURE (4 MiB) without the process being killed"
                    ),
                    mechanism=(
                        "crates/vmm/src/jail/mod.rs's armed ALLOWLIST covers mmap/munmap/madvise/mprotect/brk "
                        "and excludes mremap, with the stated ground 'the guest-memory regions are mapped once; "
                        "nothing remaps'. The serial capture is a growing Vec: glibc's realloc calls mremap for "
                        "a chunk past its mmap threshold (128 KiB by default), which fires the shipped "
                        "SECCOMP_RET_KILL_PROCESS. The derivation trace missed it because the traced 3-VM fleet "
                        "was short-lived. In the shipped in-process mode the same thread is a thread of noded, "
                        "so the death would be the whole daemon's"
                    ),
                    evidence=[str(lab.noded_log)],
                    suspected_component="crates/vmm/src/jail/mod.rs (the ALLOWLIST's allocator group)",
                    reproducer=(
                        "scripts/acceptance.sh jail  ->  the case 'the jail admits the allocator ...'; or by "
                        "hand: APZ_VMM_SPAWN=process noded --workspace <image>, POST /v1/runs n=1, then "
                        "send `wspace <text>` until the run view's serial passes 128 KiB (the VM goes killed "
                        "at ~131 KiB and journalctl -k shows syscall=25 code=0x80000000 for the child)"
                    ),
                )

    _guarded(case, body)


def run(ctx) -> Suite:
    suite = Suite(
        "jail-in-the-boot-path",
        "every case passes: the jail armed on every VMM thread of a real boot, the escape hatch "
        "unconfined, a 50-VM soak with no daemon death and no kernel refusal, the per-VM process mode's "
        "children jailed and backed by the parent-death signal, an API-created run in process mode "
        "surviving its own request, and a jailed VMM thread surviving a console capture grown past "
        "glibc's mmap threshold (the mremap gap)",
        ctx,
    )
    reason = ctx.can_run("jail")
    if reason:
        suite.blocked(reason)
        return suite
    _case_armed(suite, ctx)
    _case_escape_hatch(suite, ctx)
    _case_soak(suite, ctx)
    _case_process_mode(suite, ctx)
    _case_process_api_run(suite, ctx)
    _case_grown_console_buffer(suite, ctx)
    return suite
