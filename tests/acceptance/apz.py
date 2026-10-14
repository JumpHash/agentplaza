"""Shared harness for the AgentPlaza acceptance suites.

This module is the boring plumbing the four fixed suites
(``suite_demo``, ``suite_redteam``, ``suite_killresume``, ``suite_budgets``)
share. It was written to be runnable on every CI machine class with nothing but
CPython's standard library.

* **Why a Python runner, not a Rust bin.** The repository's own test trees are
  shell entry points around Python/Node suites (``docs/protocol-draft/check.sh``
  + ``tests/test_protocol.py``, ``docs/orchestrator-draft/check.sh`` +
  ``tests/core.test.mjs``). ``tests/acceptance`` is therefore *not* a cargo
  workspace member: ``cargo build --workspace`` is untouched, no dependency is
  added, and the same runner executes unchanged on the x86_64, Pi 4, Pi 5 and
  Apple Silicon machines. The suites drive the *real binaries* (``noded``,
  ``vsockd``) over the *real HTTP API*; nothing here links the crates.

* **Hermeticity.** Every suite execution gets a fresh ``mktemp -d`` work
  directory with a fresh ``0700`` vsock UDS directory, fresh daemons bound to
  ephemeral localhost ports, and fresh guest CIDs inside that directory. The
  host's live daemons (``noded`` on 8899, the persistent ``vsockd`` fleet) are
  never touched: the default UDS root is not ``/tmp/plaza-vsock``.

* **Machine class.** The four CI targets are ``server`` (x86_64 Linux),
  ``pi5``/``pi4`` (aarch64 Linux) and ``apple`` (aarch64 macOS) — the classes
  ``docs/orchestrator-draft/schema.sql`` ``core.machines.class`` allows. A suite
  whose class needs hardware this host does not have is reported BLOCKED with
  the reason, never silently skipped and never counted as a pass.
"""

from __future__ import annotations

import hashlib
import json
import math
import os
import re
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path


# --------------------------------------------------------------------------- paths

REPO = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
RESULTS_DIR = HERE / "results"
DEFAULT_BIN_DIR = REPO / "target" / "debug"
DEFAULT_KERNEL = Path("/boot/vmlinuz-linux")
DEFAULT_INITRD = REPO / "target" / "guest" / "initrd.x86_64.gz"
#: The base image of the guests' writable ``/workspace`` volume
#: (``crates/vmm/src/devices/virtio/block``), built by
#: ``scripts/mk-workspace-image.sh``. The budgets suite's ``--workspace`` cells
#: build it when it is absent.
DEFAULT_WORKSPACE = REPO / "target" / "guest" / "workspace.ext4"

#: The runner's own version line, recorded in every manifest.
HARNESS_VERSION = "tests/acceptance 1"


# ------------------------------------------------------------- machine classes

#: The classes ``core.machines.class`` allows (``docs/orchestrator-draft/schema.sql``).
MACHINE_CLASSES = ("server", "pi4", "pi5", "apple")

#: What each class runs on: (os, arch). ``noded``'s own machine view is the
#: runtime source of truth; this table is what decides whether a suite can run
#: *here*.
CLASS_PLATFORM = {
    "server": ("linux", "x86_64"),
    "pi4": ("linux", "aarch64"),
    "pi5": ("linux", "aarch64"),
    "apple": ("macos", "aarch64"),
}


def host_platform() -> tuple[str, str]:
    """This host's ``(os, arch)`` in the schema's spelling."""
    system = {"linux": "linux", "darwin": "macos"}.get(sys.platform, sys.platform)
    machine = {"x86_64": "x86_64", "amd64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64"}.get(
        os.uname().machine
    )
    return system, machine


def detect_machine_class() -> str | None:
    """The class this host *is*, when that is unambiguous (``auto``)."""
    system, arch = host_platform()
    for name, platform in CLASS_PLATFORM.items():
        if platform == (system, arch) and name == "server":
            return name
    if (system, arch) == ("macos", "aarch64"):
        return "apple"
    # aarch64 Linux could be a Pi 4 or a Pi 5 (or another arm64 host): the
    # class decides the irqchip and GIC validation, so the caller names it.
    return None


# ------------------------------------------------------------------- fixed bars
#
# These are the suite's own constants, never passed in (that is what makes the
# demo matrix "hidden") and never softened. Provenance is noted for each.

#: Whole-fleet host RSS budget for the 50-VM run, in bytes.
#: Source: the port record, slice 16 (owner criterion "50 microVMs < 8 GiB";
#: validated 5.57 GiB).
FLEET_RSS_BUDGET = 8 << 30

#: Steady-state CPU budget for the same fleet, as a percentage of one core.
#: Source: the port record, slice 16 ("low CPU"); the validated figure there is
#: 0.03 % with the guests parked in HLT.
FLEET_CPU_BUDGET_PCT = 5.0

#: The VMM's own kill deadline (``crates/vmm/src/spawner.rs`` ``KILL_TIMEOUT``).
#: A ``DELETE`` that does not tear its VMs down inside this is a failure.
KILL_TIMEOUT_S = 10.0

#: The in-memory serial capture cap (``crates/vmm/src/devices/legacy/serial.rs``
#: ``MAX_SERIAL_CAPTURE``).
SERIAL_CAPTURE_CAP = 4 << 20

#: The frozen orchestrator port (the guest harness and the muxer both use it).
VSOCK_PORT = 5000

#: How long one demo/red-team run may take from POST to a final state.
RUN_DEADLINE_S = 120.0

#: How long one 50-VM run may take from POST to a final state.
FLEET_DEADLINE_S = 300.0

#: How long the whole runner waits for a daemon to print its bound port.
START_DEADLINE_S = 20.0

#: Slack when attributing a run's workspace directory to a lab by mtime: a
#: daemon creates `run-{id}-{epoch_ms}` well after the lab records its start,
#: so this only absorbs filesystem-timestamp jitter.
RUN_DIR_CLOCK_SLACK_S = 1.0


# ------------------------------------------------------------------ small utils



def now() -> float:
    return time.monotonic()


def iso_now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def percentile(values: list[float], fraction: float) -> float | None:
    """The nearest-rank percentile of ``values`` (``None`` for an empty list)."""
    if not values:
        return None
    ordered = sorted(values)
    rank = max(1, min(len(ordered), math.ceil(fraction * len(ordered))))
    return ordered[rank - 1]


def summary(values: list[float]) -> dict:
    """``{n, min, p50, p95, p99, max, mean}`` for a sample (seconds or bytes)."""
    if not values:
        return {"n": 0}
    return {
        "n": len(values),
        "min": min(values),
        "p50": percentile(values, 0.50),
        "p95": percentile(values, 0.95),
        "p99": percentile(values, 0.99),
        "max": max(values),
        "mean": sum(values) / len(values),
    }


def git_revision() -> dict:
    """The SUT/harness revision: HEAD, dirty flag, and the test tree's hash."""
    def run(*args: str) -> str:
        try:
            out = subprocess.run(
                ["git", *args], cwd=REPO, capture_output=True, text=True, timeout=20, check=False
            )
            return out.stdout.strip()
        except (OSError, subprocess.SubprocessError):
            return ""
    tree = hashlib.sha256()
    for path in sorted(HERE.glob("**/*.py")):
        tree.update(path.relative_to(HERE).as_posix().encode())
        tree.update(sha256_file(path).encode())
    return {
        "head": run("rev-parse", "HEAD"),
        "dirty": bool(run("status", "--porcelain")),
        "harness_tree_sha256": tree.hexdigest(),
    }


def meminfo_bytes(field: str) -> int | None:
    """A ``/proc/meminfo`` field in bytes, or ``None`` where the host has none.

    ``Cached`` (and ``Shmem``) is what a workspace volume's image copies cost
    the *host*: each copy is written once and its pages then live in the page
    cache (for a file under a tmpfs root, in ``Shmem`` and so also in
    ``Cached``), not in the daemon's resident set. The field is a whole-host
    figure, so the suite samples it as a delta around a synchronised point and
    reports the raw numbers rather than attributing them.
    """
    try:
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith(f"{field}:"):
                return int(line.split()[1]) * 1024
    except (OSError, ValueError, IndexError):
        return None
    return None


def child_processes(pid: int) -> list[int]:
    """Every live child of ``pid`` (noded spawns none: its VMs are threads)."""
    children = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text()
        except OSError:
            continue
        # comm may contain spaces and parentheses; PPID is field 4 after the
        # final ')'.
        tail = stat.rsplit(")", 1)[-1].split()
        if len(tail) > 1 and tail[1] == str(pid):
            children.append(int(entry.name))
    return children


# ------------------------------------------------------------- the jail's view
#
# The seccomp jail (``crates/vmm/src/jail``) is armed *per VMM thread*, not
# once for the daemon: ``noded`` runs each VM's vCPU, vsock muxer and exit
# watcher as threads of its own process, and each arms ``SECCOMP_MODE_FILTER``
# on itself just before its loop. The three thread kinds are named at spawn
# (``apz_vcpu_{n}``, ``apz_vm{id}_vsock``, ``apz_vm{id}_exit``), so the kernel's
# own per-thread ``Seccomp:`` field is what proves the filter is on — no
# daemon's word is needed, and none is trusted.

#: The thread-name prefixes ``crates/vmm`` gives the threads it arms. Anything
#: else in the daemon (its request loop, a KVM-created helper) is not one of
#: the armed kinds.
VMM_THREAD_PREFIXES = ("apz_vcpu_", "apz_vm")

#: The kernel's ``Seccomp:`` field values (``/proc/<tid>/status``, ``linux/sched.h``).
SECCOMP_DISABLED = "0"
SECCOMP_FILTER = "2"


def thread_status(pid: int) -> list[dict]:
    """Every thread of ``pid``, with the kernel's own view of its filter.

    ``Seccomp`` is the field ``/proc/<tid>/status`` reports: ``0`` none, ``1``
    strict, ``2`` filter — the kernel's record of the filter installed on that
    *thread*, which is the only honest way to say the jail is on. Reading a
    daemon's ``/proc/<pid>/status`` would report only its leader thread (the
    request loop), which is deliberately *not* armed, so a per-thread scan is
    required rather than convenient.
    """
    threads: list[dict] = []
    try:
        entries = list(Path(f"/proc/{pid}/task").iterdir())
    except OSError:
        return threads
    for entry in entries:
        name = seccomp = "?"
        try:
            for line in (entry / "status").read_text(errors="replace").splitlines():
                if line.startswith("Name:"):
                    name = line.split(":", 1)[1].strip()
                elif line.startswith("Seccomp:"):
                    seccomp = line.split(":", 1)[1].strip()
        except OSError:
            continue
        threads.append({"tid": int(entry.name), "name": name, "seccomp": seccomp})
    return sorted(threads, key=lambda item: item["tid"])


def vmm_threads(pid: int) -> list[dict]:
    """The threads of ``pid`` the jail arms — the ``apz_*`` VMM threads."""
    return [thread for thread in thread_status(pid) if thread["name"].startswith(VMM_THREAD_PREFIXES)]


def await_vmm_threads(pid: int, expect: int, timeout: float = 10.0) -> list[dict]:
    """Wait until at least ``expect`` VMM threads are up (their filter armed)."""
    deadline = now() + timeout
    threads = vmm_threads(pid)
    while len(threads) < expect and now() < deadline:
        time.sleep(0.05)
        threads = vmm_threads(pid)
    return threads


def kernel_seccomp_audit(since_epoch: float) -> dict:
    """The kernel's seccomp audit records (``type=1326``) since ``since_epoch``.

    A ``SECCOMP_RET_KILL_PROCESS`` refusal is an ``audit: type=1326`` record
    with ``code=0x80000000``; the ``log`` arm's allowed refusal is
    ``code=0x7ffc0000`` (see ``crates/vmm/src/jail``). The kernel writes them to
    the audit trail, which ``journalctl -k`` reads on this host. The raw lines
    are returned, so the evidence is the kernel's own and not a paraphrase.
    """
    command = ["journalctl", "-k", "--since", f"@{int(since_epoch)}", "--no-pager", "-o", "short-iso"]
    try:
        out = subprocess.run(command, capture_output=True, text=True, timeout=60, check=False)
    except (OSError, subprocess.SubprocessError) as err:
        return {"available": False, "reason": f"{type(err).__name__}: {err}", "lines": [], "kills": []}
    if out.returncode != 0:
        reason = out.stderr.strip() or f"journalctl exited {out.returncode}"
        return {"available": False, "reason": reason, "lines": [], "kills": []}
    lines = [line for line in out.stdout.splitlines() if "type=1326" in line]
    kills = [line for line in lines if "code=0x80000000" in line]
    return {"available": True, "since_epoch": int(since_epoch), "lines": lines, "kills": kills}


def vmm_audit_attribution(line: str, bin_dir: Path | None = None) -> str | None:
    """Why a ``type=1326`` line is *this run's* VMM, or ``None``.

    ``crates/vmm/tests/jail.rs`` makes ``the_probe_child`` records *on purpose*
    (it arms the pre-fix allowlist to show what it refused), so those are not
    attributed: their comm is the probe's own name and their exe is a test
    binary, not one of the harness's.

    Everything else the kernel names as a VMM thread (``comm="apz_*"``), a
    KVM-created thread that inherited an armed parent's filter
    (``comm="kvm-nx-lpage-re"``), or one of the daemon binaries is a syscall one
    of this port's VMM threads reached that the allowlist did not cover —
    **provided it is this run's binary**. When ``bin_dir`` is given, a record is
    attributed only when the ``exe`` it names is a binary of that directory:
    the thread and binary *names* are not unique on a shared host. Found
    2026-10-11: the matrix, whose SUT was a pinned copy in
    ``/mnt/fast/apz-accept/pin``, attributed two refusals from the live stack's
    ``/mnt/fast/AgentPlaza/target/debug/apz-vmm`` children to the run under
    test and reported them as its own daemon deaths (the runner's verdict went
    to FAIL and a product-defect finding named the wrong process). ``bin_dir``
    is optional so a caller that really does mean "any VMM on this host" can
    still ask for that, but every harness caller passes it.
    """
    comm = re.search(r'comm="([^"]*)"', line)
    exe = re.search(r'exe="([^"]*)"', line)
    name = comm.group(1) if comm else ""
    binary = Path(exe.group(1)).name if exe else ""
    if bin_dir is not None:
        if not exe:
            return None
        try:
            if Path(exe.group(1)).resolve().parent != Path(bin_dir).resolve():
                return None
        except OSError:
            return None
    if name.startswith("apz_") or name == "kvm-nx-lpage-re":
        return f"comm={name}"
    if binary in ("noded", "vsockd", "vmctl"):
        return f"exe={binary}"
    return None


# ------------------------------------------------------- the SUT's own binary
#
# Every suite spawns its daemons from ``bin_dir`` lazily, one lab at a time, so
# the binaries must not change *while* the matrix runs: a `cargo build` that
# relinks `target/debug/noded` mid-run hands the later suites a different SUT
# than the earlier ones, and the run's verdict is then about no single build.

#: The binaries a suite spawns: the node daemon and the hub.
HARNESS_BINARIES = ("noded", "vsockd")


def binary_digests(bin_dir: Path) -> dict[str, str]:
    """The sha256 of every harness binary in ``bin_dir``, keyed by name."""
    return {
        name: sha256_file(bin_dir / name) for name in HARNESS_BINARIES if (bin_dir / name).exists()
    }


def binary_digest_changes(before: dict[str, str], after: dict[str, str]) -> dict[str, tuple]:
    """``{name: (before, after)}`` for every binary whose bytes changed.

    Found on 2026-10-11: a sibling's `cargo build --workspace` relinked the
    shared ``target/debug/noded`` at 08:52:43 during a full matrix, so the tail
    suites ran a binary built from uncommitted ``crates/vmm`` work while the
    head suites ran the committed one — and the manifest, computed at the end,
    recorded only the *last* build. Such a run is not a result about one SUT:
    it is reported as a FAILURE with both digests, never averaged into a
    verdict. ``--bin-dir`` on a pinned copy makes a run hermetic.
    """
    return {
        name: (before.get(name), after.get(name))
        for name in sorted(set(before) | set(after))
        if before.get(name) != after.get(name)
    }


# ------------------------------------------------------------ credential scan
#: Credential-shaped patterns the leak scan hunts for on every surface. None of
#: these can legitimately appear in an MVP transcript: there are no credentials
#: in the system at all (decision 0010).
CREDENTIAL_PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("aws-access-key-id", re.compile(r"AKIA[0-9A-Z]{16}")),
    ("private-key-block", re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----")),
    ("github-token", re.compile(r"gh[pousr]_[A-Za-z0-9]{20,}")),
    ("openai-style-key", re.compile(r"\bsk-[A-Za-z0-9_-]{20,}")),
    ("slack-token", re.compile(r"xox[baprs]-[A-Za-z0-9-]{10,}")),
    ("jwt", re.compile(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}")),
    (
        "assignment-shaped",
        re.compile(r"(?i)\b(api[_-]?key|secret[_-]?key|password|passwd|access[_-]?token)\b\s*[:=]\s*\S{8,}"),
    ),
)


def credential_hits(text: str) -> list[str]:
    """Every credential-shaped pattern that appears in ``text``."""
    return [name for name, pattern in CREDENTIAL_PATTERNS if pattern.search(text)]


def probe_unix_socket(path: Path) -> str:
    """Connect to a Unix socket path and classify what happened.
    ``answers`` — something is listening; ``refused`` — a socket file with
    nothing behind it (a stale path); ``missing`` — no socket file at all.
    """
    probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    probe.settimeout(2.0)
    try:
        probe.connect(str(path))
        return "answers"
    except FileNotFoundError:
        return "missing"
    except ConnectionRefusedError:
        return "refused"
    except OSError as err:
        return f"error:{err.errno}"
    finally:
        probe.close()

#: A kernel console line: the printk timestamp prefix. The guest's console is
#: one serial FIFO that three writers share (the kernel, the init and the agent
#: harness), so a single userspace write can be split *inside* a line by a
#: kernel chunk, or by the other userspace process.
KERNEL_LINE = re.compile(r"\[\s*\d+\.\d+\][^\n]*\n?")


def reconstruct_console(serial: str) -> str:
    """The userspace console stream, with the kernel's interleaved chunks removed.
    Because the three console writers share one FIFO, a line one writer emitted
    contiguously can appear in the raw capture split across several physical
    lines by another writer's output. Dropping the kernel's printk chunks and
    the line breaks re-concatenates each writer's stream in byte order, so a
    string one writer emitted contiguously is a substring again. Checks that
    must not be fooled by the interleaving use this view *in addition to* the
    raw capture (raw catches anything the kernel itself printed).
    """
    return KERNEL_LINE.sub("", serial).replace("\r\n", "\n").replace("\n", "")


# ------------------------------------------------------------------ the daemons


class LabError(RuntimeError):
    """A lab could not be brought up or driven."""


def work_dir_reuse_reason(path: Path) -> str | None:
    """Why ``path`` cannot be this run's work directory, or ``None``.

    The harness's hermeticity contract is one **fresh** work directory per run
    (``--artifacts`` of its own, or none for a ``mktemp -d``), and a reused one
    is refused rather than used, because its leftovers are dangerous rather than
    merely untidy: a lab opens its daemon logs for *append* and
    ``Lab._await_line`` scans the whole file, so a previous run's
    ``noded: listening on http://127.0.0.1:PORT`` line satisfies the wait before
    the new daemon has printed anything and the harness then talks to a dead
    port; a previous run's per-CID socket files satisfy ``Lab._await_listeners``
    the same way, and a stale ``vm{cid}`` path can even make a fresh muxer's
    bind fail. Found on 2026-10-11: a full-matrix run pointed ``--artifacts`` at
    a previous run's directory, every demo cell and the red-team lab reported
    ``Connection refused`` against the stale port, and the run's own results
    would have mixed two runs' artifacts.
    """
    if not path.exists():
        return None
    try:
        leftover = sorted(entry.name for entry in path.iterdir())
    except OSError as err:
        return f"it cannot be read: {err}"
    if not leftover:
        return None
    # Name what was actually found: a lab's logs (`{tag}-logs`) and its socket
    # directory (`{tag}-uds`) are the two leftovers that defeat the readiness
    # waits, and saying so is what makes the refusal actionable.
    logs = [name for name in leftover if name.endswith("-logs")]
    sockets = [name for name in leftover if name.endswith("-uds")]
    found = []
    if logs:
        found.append(
            f"the daemon logs {logs[0]!r} (a stale `listening on` line satisfies a readiness wait "
            f"and the harness then talks to a dead port)"
        )
    if sockets:
        found.append(f"the socket directory {sockets[0]!r} (a stale socket path is not a live listener)")
    detail = "; ".join(found) or f"entries starting {leftover[0]!r}"
    return (
        f"it already holds leftovers from an earlier run: {detail} — a reused directory silently "
        f"defeats the harness's readiness waits, so this run's results would mix two runs' state"
    )


class Result:
    """One HTTP reply: status plus decoded JSON body (when it was JSON)."""
    def __init__(self, status: int, body: str, elapsed: float):
        self.status = status
        self.body = body
        self.elapsed = elapsed
    @property
    def json(self) -> dict:
        return json.loads(self.body)
    def __repr__(self) -> str:
        return f"Result(status={self.status}, body={self.body[:200]!r})"


class Lab:
    """A hermetic lab: one work directory, one ``vsockd``, one ``noded``.
    The daemons are started in their own session (``start_new_session=True``)
    so they survive a backgrounded runner and can be signalled as a group;
    their stdout/stderr go to files inside the work directory, which is the
    artifact store for the suite.
    """
    def __init__(
        self,
        ctx: "Ctx",
        cids: list[int],
        tag: str,
        *,
        canary_env: dict[str, str] | None = None,
        env: dict[str, str] | None = None,
        uds_dir: Path | None = None,
        workspace: Path | None = None,
        status_port: int | None = None,
        admin_port: int | None = None,
        roles: Path | None = None,
        agents: Path | None = None,
        agents_cid_base: int | None = None,
        hub_status: str | None = None,
        hub_admin: str | None = None,
    ):
        self.ctx = ctx
        self.tag = tag
        # A restarted daemon must come back on the *same* UDS directory for a
        # restart to mean anything, so the caller may pass the one the killed
        # daemon used instead of a fresh directory.
        self.uds_dir = Path(uds_dir) if uds_dir is not None else ctx.new_dir(f"{tag}-uds", mode=0o700)
        if uds_dir is not None:
            self.uds_dir.mkdir(parents=True, exist_ok=True)
            os.chmod(self.uds_dir, 0o700)
        self.log_dir = ctx.new_dir(f"{tag}-logs")
        self.cids = list(cids)
        self.canary_env = dict(canary_env or {})
        # Extra environment for the daemons this lab spawns, merged over
        # ``os.environ`` (and over the canaries): how a case configures a
        # daemon's own switch — the jail suite's ``APZ_VMM_NO_JAIL=1`` arm is
        # the reason it exists, and it keeps that switch out of the *process*
        # environment, where it would leak into every later lab.
        self.extra_env = dict(env or {})
        # The base image of every run's writable workspace volume, or `None`
        # for a daemon that boots no block device (`noded --workspace`).
        self.workspace = Path(workspace) if workspace is not None else None
        # The hub's read-only status surface (`vsockd --status-port`), when the
        # caller asks for it: `0` binds an ephemeral port, whose real number is
        # parsed from the daemon's log line (`self.status_bound`).
        self.status_port = status_port
        self.status_bound: int | None = None
        # The hub's admin surface (`vsockd --admin-port`), when the caller asks
        # for it: `0` binds an ephemeral port, whose real number is parsed from
        # the daemon's log line (`self.admin_bound`).
        self.admin_port = admin_port
        self.admin_bound: int | None = None
        # The roles manifest the hub enforces the codex's tool grants from
        # (`vsockd --roles`), or `None` for a daemon with no policy.
        self.roles = Path(roles) if roles is not None else None
        # The roles manifest the daemon boots its initial fleet from
        # (`noded --agents`), or `None` for a daemon with no fleet.
        self.agents = Path(agents) if agents is not None else None
        # The first guest CID of that fleet (`noded --agents-cid-base`); a role
        # with slot `s` gets `agents_cid_base + s` (its alphabetical index when
        # the manifest has no slots).
        self.agents_cid_base = agents_cid_base
        # Where `noded` proxies the hub's status surface (`--hub-status`):
        # the literal `HOST:PORT`, the sentinel `"self"` for this lab's own
        # vsockd status port, or `None` to leave the flag off.
        self.hub_status = hub_status
        # Where `noded` drives the hub's admin surface (`--hub-admin`): the
        # literal `HOST:PORT`, the sentinel `"self"` for this lab's own vsockd
        # admin port, or `None` to leave the flag off.
        self.hub_admin = hub_admin
        self.noded: subprocess.Popen | None = None
        self.vsockd: subprocess.Popen | None = None
        self.port: int = 0
        self.noded_log = self.log_dir / "noded.log"
        self.vsockd_log = self.log_dir / "vsockd.log"
        self.started_at = iso_now()
        # The lab's own lifetime, as wall-clock bounds: a run's workspace
        # directory is attributed to the daemon that created it by its mtime
        # (see `run_dir`), which matters when two daemons share one UDS root.
        self.started_wall = time.time()
        self.stopped_wall: float | None = None
        ctx.register_artifact(self.noded_log)
        ctx.register_artifact(self.vsockd_log)
    # -- lifecycle ---------------------------------------------------------
    def __enter__(self) -> "Lab":
        return self.start()
    def __exit__(self, *_exc) -> None:
        self.stop()
    def start(self) -> "Lab":
        argv_noded = [
            str(self.ctx.bin("noded")),
            "--port",
            "0",
            "--uds-dir",
            str(self.uds_dir),
            "--kernel",
            str(self.ctx.kernel),
            "--initrd",
            str(self.ctx.initrd),
        ]
        if self.workspace is not None:
            # The daemon gives every VM of every run its own copy of this base
            # image and boots it as the guest's writable `/workspace` volume.
            argv_noded += ["--workspace", str(self.workspace)]
        if self.cids:
            argv_vsockd = [
                str(self.ctx.bin("vsockd")),
                "--uds-dir",
                str(self.uds_dir),
                "--cids",
                ",".join(str(cid) for cid in self.cids),
                "--port",
                str(VSOCK_PORT),
            ]
            if self.status_port is not None:
                # The read-only registry surface, on an ephemeral port by
                # default (`--status-port 0`).
                argv_vsockd += ["--status-port", str(self.status_port)]
            if self.admin_port is not None:
                # The control plane's admin surface (`POST /tell`,
                # `POST /stop`), ephemeral by default (`--admin-port 0`).
                argv_vsockd += ["--admin-port", str(self.admin_port)]
            if self.roles is not None:
                # The codex's per-role tool grants, enforced on every
                # `/tell` the admin surface accepts.
                argv_vsockd += ["--roles", str(self.roles)]
            self.vsockd = self._spawn("vsockd", argv_vsockd, self.vsockd_log)
            self._await_line(self.vsockd_log, "listening on")
            # The daemon binds its listeners one CID at a time, so wait for
            # every one of them: a probe or a UDS enumeration before the last
            # bind would see a half-started daemon.
            self._await_listeners()
            if self.status_port is not None:
                self._await_status_port()
            if self.admin_port is not None:
                self._await_admin_port()
        if self.agents is not None:
            # The daemon boots its initial role fleet from this manifest before
            # it serves, one long-lived keepalive VM per role.
            argv_noded += ["--agents", str(self.agents)]
            if self.agents_cid_base is not None:
                # A role's guest CID is this base plus its slot (its alphabetical
                # index when the manifest has no slots), so the caller knows which
                # CID a role's guest connects as.
                argv_noded += ["--agents-cid-base", str(self.agents_cid_base)]
        if self.hub_status is not None:
            # `noded --hub-status` points the daemon's `/api/v0/hub` proxy at
            # the hub's status surface: the sentinel `"self"` resolves to this
            # lab's own vsockd, anything else is passed through (`off`, or a
            # literal HOST:PORT — a closed port exercises the 502 path).
            if self.hub_status == "self":
                if self.status_bound is None:
                    raise LabError("hub_status='self' needs the lab's own status_port")
                argv_noded += ["--hub-status", f"127.0.0.1:{self.status_bound}"]
            else:
                argv_noded += ["--hub-status", str(self.hub_status)]
        if self.hub_admin is not None:
            # `noded --hub-admin` points the daemon's
            # `/api/v0/agents/{cid}/tell` and `/api/v0/tasks/{id}/artifact` at
            # the hub's admin surface. The sentinel `"self"` resolves to this
            # lab's own vsockd admin port; anything else is passed through
            # (`off`, or a literal HOST:PORT).
            if self.hub_admin == "self":
                if self.admin_bound is None:
                    raise LabError("hub_admin='self' needs the lab's own admin_port")
                argv_noded += ["--hub-admin", f"127.0.0.1:{self.admin_bound}"]
            else:
                argv_noded += ["--hub-admin", str(self.hub_admin)]
        self.noded = self._spawn("noded", argv_noded, self.noded_log)
        line = self._await_line(self.noded_log, "listening on")
        match = re.search(r"127\.0\.0\.1:(\d+)", line)
        if not match:
            raise LabError(f"noded did not report a port: {line!r}")
        self.port = int(match.group(1))
        return self
    def stop(self) -> None:
        for process in (self.noded, self.vsockd):
            if process is not None and process.poll() is None:
                try:
                    os.killpg(os.getpgid(process.pid), signal.SIGTERM)
                except (ProcessLookupError, PermissionError):
                    pass
        for process in (self.noded, self.vsockd):
            if process is not None:
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(os.getpgid(process.pid), signal.SIGKILL)
                    except (ProcessLookupError, PermissionError):
                        pass
        self.noded = self.vsockd = None
        # The lab's lifetime closes here: after this, a run directory it
        # created is still *its* (a finished run's copies are kept), but a
        # later lab's are not.
        if self.stopped_wall is None:
            self.stopped_wall = time.time()
    def _spawn(self, name: str, argv: list[str], log: Path) -> subprocess.Popen:
        handle = open(log, "ab", buffering=0)
        try:
            return subprocess.Popen(
                argv,
                stdout=handle,
                stderr=subprocess.STDOUT,
                stdin=subprocess.DEVNULL,
                start_new_session=True,
                env={**os.environ, **self.canary_env, **self.extra_env},
            )
        except OSError as err:
            raise LabError(f"cannot start {name}: {err}") from err

    def _await_listeners(self) -> None:
        """Wait until every served CID's listener socket exists."""
        expected = {f"vm{cid}_{VSOCK_PORT}" for cid in self.cids}
        deadline = now() + START_DEADLINE_S
        while now() < deadline:
            if expected <= set(self.uds_entries()):
                return
            if self.vsockd is not None and self.vsockd.poll() is not None:
                raise LabError(f"vsockd exited while binding listeners:\n{self.vsockd_log_text()}")
            time.sleep(0.02)
        missing = sorted(expected - set(self.uds_entries()))
        raise LabError(f"vsockd never bound {missing}")

    def _await_status_port(self) -> None:
        """Wait for the daemon's status line and record the port it bound.

        ``--status-port 0`` binds an ephemeral port, so the real number is only
        known once the daemon prints it.
        """
        line = self._await_line(self.vsockd_log, "status on http")
        match = re.search(r"127\.0\.0\.1:(\d+)", line)
        if not match:
            raise LabError(f"vsockd did not report a status port: {line!r}")
        self.status_bound = int(match.group(1))

    def _await_admin_port(self) -> None:
        """Wait for the daemon's admin line and record the port it bound.

        ``--admin-port 0`` binds an ephemeral port, so the real number is only
        known once the daemon prints it (``vsockd: admin on http://...``).
        """
        line = self._await_line(self.vsockd_log, "admin on http")
        match = re.search(r"127\.0\.0\.1:(\d+)", line)
        if not match:
            raise LabError(f"vsockd did not report an admin port: {line!r}")
        self.admin_bound = int(match.group(1))

    def _await_line(self, log: Path, needle: str) -> str:
        deadline = now() + START_DEADLINE_S
        while now() < deadline:
            text = log.read_text(errors="replace") if log.exists() else ""
            for line in text.splitlines():
                if needle in line:
                    return line
            if (self.noded is not None and self.noded.poll() is not None) or (
                self.vsockd is not None and self.vsockd.poll() is not None
            ):
                raise LabError(f"a daemon exited before '{needle}':\n{text}")
            time.sleep(0.05)
        raise LabError(f"timed out waiting for {needle!r} in {log}")
    # -- HTTP API ----------------------------------------------------------
    def _request(self, method: str, path: str, body: dict | None = None, timeout: float = 300.0) -> Result:
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(
            f"http://127.0.0.1:{self.port}{path}", method=method, data=data
        )
        if data is not None:
            request.add_header("Content-Type", "application/json")
        started = now()
        try:
            with urllib.request.urlopen(request, timeout=timeout) as reply:
                return Result(reply.status, reply.read().decode(), now() - started)
        except urllib.error.HTTPError as err:
            return Result(err.code, err.read().decode(), now() - started)
    def machine(self) -> dict:
        return self._request("GET", "/api/v0/machine").json
    def api_request(self, method: str, path: str, body: dict | None = None, timeout: float = 300.0) -> "Result":
        """One request to ``noded``'s own HTTP API (the raw reply)."""
        return self._request(method, path, body, timeout)
    def api_get(self, path: str) -> "Result":
        """``GET`` a path on ``noded``'s own HTTP API (the raw reply)."""
        return self.api_request("GET", path)
    def api_post(self, path: str, body: dict | None = None, timeout: float = 300.0) -> "Result":
        """``POST`` a JSON body to ``noded``'s own HTTP API (the raw reply)."""
        return self.api_request("POST", path, body, timeout)
    def api_delete(self, path: str) -> "Result":
        """``DELETE`` a path on ``noded``'s own HTTP API (the raw reply)."""
        return self.api_request("DELETE", path)
    # -- the hub's admin surface -------------------------------------------
    def admin_request(self, method: str, path: str, body: dict | None = None, timeout: float = 20.0) -> "Result":
        """One request to the hub's admin surface (``POST /tell``, ``POST /stop``)."""
        if self.admin_bound is None:
            raise LabError("this lab has no admin surface (start it with admin_port=...)")
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(
            f"http://127.0.0.1:{self.admin_bound}{path}", method=method, data=data
        )
        if data is not None:
            request.add_header("Content-Type", "application/json")
        started = now()
        try:
            with urllib.request.urlopen(request, timeout=timeout) as reply:
                return Result(reply.status, reply.read().decode(), now() - started)
        except urllib.error.HTTPError as err:
            return Result(err.code, err.read().decode(), now() - started)
    def admin_tell(self, cid: int, name: str, args: dict | None = None, timeout_ms: int | None = None) -> "Result":
        """``POST /tell``: drive one tool call on the agent connected as ``cid``."""
        body: dict = {"cid": cid, "name": name}
        if args is not None:
            body["args"] = args
        if timeout_ms is not None:
            body["timeout_ms"] = timeout_ms
        return self.admin_request("POST", "/tell", body)
    def admin_stop(self, cid: int, reason: str | None = None) -> "Result":
        """``POST /stop``: ask the agent connected as ``cid`` to end its session."""
        body: dict = {"cid": cid}
        if reason is not None:
            body["reason"] = reason
        return self.admin_request("POST", "/stop", body)
    # -- the hub's read-only status surface --------------------------------
    def status(self) -> dict:
        """``GET /status`` — the hub's registry, from its read-only surface."""
        return self.status_get("/status").json
    def healthz(self) -> "Result":
        """``GET /healthz`` — the status surface's liveness probe."""
        return self.status_get("/healthz")
    def status_get(self, path: str, timeout: float = 5.0) -> "Result":
        """One request to the hub's read-only status surface (``/status`` + ``/healthz``)."""
        if self.status_bound is None:
            raise LabError("this lab has no status surface (start it with status_port=...)")
        request = urllib.request.Request(f"http://127.0.0.1:{self.status_bound}{path}")
        started = now()
        try:
            with urllib.request.urlopen(request, timeout=timeout) as reply:
                return Result(reply.status, reply.read().decode(), now() - started)
        except urllib.error.HTTPError as err:
            return Result(err.code, err.read().decode(), now() - started)
    def ping(self) -> bool:
        """Whether the daemon is still answering its API."""
        try:
            return self._request("GET", "/api/v0/machine").status == 200
        except (OSError, ValueError):
            return False
    def stream_events(self, seconds: float, sink: list[str] | None = None) -> list[str]:
        """Consume ``GET /api/v0/events`` for ``seconds`` and return its lines.

        ``sink``, when given, receives every line **as it arrives**: the return
        value is only complete once the read ends, so a caller that must inspect
        the stream while it is still open — a session-long frame capture — polls
        the sink instead.
        """
        connection = socket.create_connection(("127.0.0.1", self.port), timeout=5)
        connection.sendall(
            b"GET /api/v0/events HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\n\r\n"
        )
        connection.settimeout(0.25)
        deadline = now() + seconds
        lines: list[str] = []
        buffer = b""
        while now() < deadline:
            try:
                chunk = connection.recv(1 << 16)
            except TimeoutError:
                continue
            except OSError:
                break
            if not chunk:
                break
            buffer += chunk
            while b"\n" in buffer:
                raw, buffer = buffer.split(b"\n", 1)
                line = raw.decode(errors="replace")
                lines.append(line)
                if sink is not None:
                    sink.append(line)
        connection.close()
        return lines
    def list_runs(self) -> list[dict]:
        return self._request("GET", "/api/v0/runs").json["runs"]
    def create(self, **spec) -> tuple[Result, dict]:
        reply = self._request("POST", "/api/v0/runs", spec)
        if reply.status != 201:
            raise LabError(f"POST /api/v0/runs refused: {reply.status} {reply.body}")
        return reply, reply.json
    def get(self, run_id: str) -> dict:
        return self._request("GET", f"/api/v0/runs/{run_id}").json
    def delete(self, run_id: str) -> Result:
        return self._request("DELETE", f"/api/v0/runs/{run_id}")
    def input(self, run_id: str, vm: int, data: str) -> Result:
        return self._request("POST", f"/api/v0/runs/{run_id}/input", {"data": data, "vm": vm})
    def await_state(self, run_id: str, states: set[str], deadline: float) -> tuple[dict, float]:
        while True:
            view = self.get(run_id)
            if view["state"] in states:
                return view, now()
            if now() > deadline:
                raise LabError(f"run {run_id} stayed {view['state']} past the deadline")
            time.sleep(0.05 if len(view.get("vms", [])) < 8 else 0.2)
    def await_port_open(self, timeout: float = START_DEADLINE_S) -> None:
        deadline = now() + timeout
        while now() < deadline:
            with socket.socket() as probe:
                probe.settimeout(0.5)
                if probe.connect_ex(("127.0.0.1", self.port)) == 0:
                    return
            time.sleep(0.05)
        raise LabError("noded's port never opened")
    # -- host-side observations -------------------------------------------
    def noded_log_text(self) -> str:
        return self.noded_log.read_text(errors="replace") if self.noded_log.exists() else ""
    def vsockd_log_text(self) -> str:
        return self.vsockd_log.read_text(errors="replace") if self.vsockd_log.exists() else ""
    def uds_entries(self) -> list[str]:
        return sorted(entry.name for entry in self.uds_dir.iterdir())
    def muxer_marker(self, cid: int) -> Path:
        """The path the VMM's muxer binds for guest ``cid`` (``VsockConfig::uds_path``)."""
        return self.uds_dir / f"vm{cid}"
    def listener_marker(self, cid: int, port: int = VSOCK_PORT) -> Path:
        """The path ``vsockd`` listens on for guest ``cid`` (``{uds}_{port}``)."""
        return self.uds_dir / f"vm{cid}_{port}"
    def run_dir(self, run_id: str) -> Path:
        """A run's private directory under the UDS root.

        ``noded`` names it ``{uds}/run-{id}-{epoch_ms}``: the daemon's start
        stamp is part of the name so a new ``r1`` can never overwrite a
        previous daemon's retained copies (the fix for the ``--workspace``
        sweep's finding, commit ``d275921``). An older daemon named it a bare
        ``run-{id}``, which is the fallback. When a restarted daemon shares the
        UDS root the same run id has two directories, so this returns the one
        *this* lab's daemon created — the candidate whose mtime falls inside
        the lab's own lifetime — and the other lab's stays addressable through
        its own handle. ``noded`` keeps a finished run's directory (it holds
        the guests' work), so an operator still addresses it after the run.
        """
        bare = self.uds_dir / f"run-{run_id}"
        candidates = sorted(self.uds_dir.glob(f"run-{run_id}-*"))
        if not candidates:
            return bare
        def inside_lifetime(path: Path) -> bool:
            mtime = path.stat().st_mtime
            return mtime >= self.started_wall - RUN_DIR_CLOCK_SLACK_S and (
                self.stopped_wall is None or mtime <= self.stopped_wall
            )
        own = [path for path in candidates if inside_lifetime(path)]
        return own[-1] if own else candidates[-1]

    def workspace_copies(self, run_id: str) -> list[Path]:
        """A run's workspace copies, in VM order (``workspace-{i}.ext4``)."""
        directory = self.run_dir(run_id)
        if not directory.is_dir():
            return []
        return sorted(
            (entry for entry in directory.iterdir() if entry.name.startswith("workspace-")),
            key=lambda path: path.name,
        )

    def rss(self) -> int:
        """``noded``'s resident set size, in bytes (its VMs are threads of it)."""
        for line in Path(f"/proc/{self.noded.pid}/status").read_text().splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1]) * 1024
        return 0
    def cpu_ticks(self) -> int:
        """``noded``'s user + system CPU ticks (all threads: the VMs are threads)."""
        fields = Path(f"/proc/{self.noded.pid}/stat").read_text().split()
        return int(fields[13]) + int(fields[14])
    def threads(self) -> int:
        return len(os.listdir(f"/proc/{self.noded.pid}/task"))
    def process_group_members(self) -> list[int]:
        """Every live PID in ``noded``'s own session/process group.
        The daemon is started as its own session leader and runs its VMs as
        threads, so anything else in its group would be a leaked child VMM
        process. This is scoped to the daemon, so other tenants' ``vmctl`` or
        ``qemu`` processes on a shared host are never mistaken for an orphan.
        """
        leader = self.noded.pid
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
    def af_vsock_sockets(self) -> tuple[bool, list[str]]:
        """``(table_present, matching_socket_inodes)`` for ``noded``.
        The userspace muxer is the only vsock transport, so the daemon must
        hold no ``AF_VSOCK`` socket. ``/proc/net/vsock`` lists every VSOCK
        socket on the host; a process holds one when one of its socket inodes
        appears there.
        """
        try:
            table = Path("/proc/net/vsock").read_text()
        except OSError:
            return False, []
        inodes = set()
        for fd in Path(f"/proc/{self.noded.pid}/fd").iterdir():
            try:
                target = os.readlink(fd)
            except OSError:
                continue
            match = re.match(r"socket:\[(\d+)\]", target)
            if match:
                inodes.add(match.group(1))
        matching = [line for line in table.splitlines()[1:] if line.split() and line.split()[-1] in inodes]
        return True, matching


# --------------------------------------------------------------- suite results


class Case:
    """One named case of a suite: its tasks, its invariants, its verdict."""
    def __init__(self, name: str, detail: str = "", vector: str | None = None, required_tasks: int | None = None):
        self.name = name
        self.detail = detail
        self.vector = vector
        self.required_tasks = required_tasks
        self.tasks: list[dict] = []
        self.invariants: list[dict] = []
        self.notes: list[str] = []
        #: Why this one case could not run (its input is missing, say). A
        #: blocked case is never a pass: `ok` is false and the suite reports
        #: BLOCKED, so a required input that is absent is visible in the
        #: verdict instead of being read as green.
        self.blocked_reason: str | None = None
    def note(self, text: str) -> None:
        self.notes.append(text)
    def blocked(self, reason: str) -> None:
        """Record that this case cannot run here, and why."""
        self.blocked_reason = reason
        self.notes.append(f"BLOCKED: {reason}")
    def task(self, name: str, ok: bool, detail: str = "") -> bool:
        self.tasks.append({"name": name, "ok": bool(ok), "detail": detail})
        return bool(ok)
    def invariant(self, name: str, ok: bool, detail: str = "") -> bool:
        self.invariants.append({"name": name, "ok": bool(ok), "detail": detail})
        return bool(ok)
    @property
    def tasks_passed(self) -> int:
        return sum(1 for task in self.tasks if task["ok"])
    @property
    def required(self) -> int:
        if self.required_tasks is not None:
            return self.required_tasks
        return 0 if not self.tasks else len(self.tasks)
    @property
    def ok(self) -> bool:
        if self.blocked_reason is not None:
            return False
        return self.tasks_passed >= self.required and all(item["ok"] for item in self.invariants)
    def view(self) -> dict:
        return {
            "name": self.name,
            "detail": self.detail,
            "vector": self.vector,
            "ok": self.ok,
            "blocked_reason": self.blocked_reason,
            "tasks_passed": self.tasks_passed,
            "tasks_required": self.required,
            "tasks_total": len(self.tasks),
            "tasks": self.tasks,
            "invariants": self.invariants,
            "notes": self.notes,
        }


class Suite:
    """A suite's result: its fixed bar, its cases, and a PASS/FAIL verdict."""
    def __init__(self, name: str, pass_bar: str, ctx: "Ctx"):
        self.name = name
        self.pass_bar = pass_bar
        self.ctx = ctx
        self.cases: list[Case] = []
        self.notes: list[str] = []
        self.findings: list[dict] = []
        self.data: dict = {}
        self.started_at = iso_now()
        self.duration_s = 0.0
        self.blocked_reason: str | None = None
    def case(self, name: str, detail: str = "", vector: str | None = None, required_tasks: int | None = None) -> Case:
        case = Case(name, detail, vector, required_tasks=required_tasks)
        self.cases.append(case)
        return case
    def note(self, text: str) -> None:
        self.notes.append(text)
    def finding(self, title: str, **fields) -> None:
        """Record a root-caused failure (or an honest flag) as a finding."""
        self.findings.append({"title": title, **fields})
        self.ctx.findings.append({"suite": self.name, "title": title, **fields})
    def blocked(self, reason: str) -> None:
        self.blocked_reason = reason
    @property
    def ok(self) -> bool:
        return self.status == "PASS"
    @property
    def status(self) -> str:
        if self.blocked_reason is not None:
            return "BLOCKED"
        if not self.cases:
            return "FAIL"
        # A failure outranks a blocked case: the suite only reports BLOCKED
        # when everything it did run passed and something it could not run is
        # named.
        if any(not case.ok and case.blocked_reason is None for case in self.cases):
            return "FAIL"
        if any(case.blocked_reason is not None for case in self.cases):
            return "BLOCKED"
        return "PASS"
    def view(self) -> dict:
        return {
            "suite": self.name,
            "pass_bar": self.pass_bar,
            "status": self.status,
            "blocked_reason": self.blocked_reason,
            "cases_passed": sum(1 for case in self.cases if case.ok),
            "cases_total": len(self.cases),
            "cases": [case.view() for case in self.cases],
            "data": self.data,
            "notes": self.notes,
            "findings": self.findings,
            "started_at": self.started_at,
            "duration_s": round(self.duration_s, 3),
        }


class Ctx:
    """Everything one runner invocation carries: paths, class, thresholds."""
    def __init__(self, args):
        self.repo = Path(args.repo).resolve()
        self.bin_dir = Path(args.bin_dir).resolve()
        self.kernel = Path(args.kernel).resolve()
        self.initrd = Path(args.initrd).resolve()
        self.workspace_image = Path(args.workspace_image).resolve()
        self.results_dir = Path(args.results_dir).resolve()
        self.machine_class = args.machine_class
        self.host_os, self.host_arch = host_platform()
        self.keep = bool(args.keep)
        self.seed = args.seed
        self.runner_timeout_scale = float(args.timeout_scale)
        self.artifacts_dir = Path(
            args.artifacts or tempfile.mkdtemp(prefix=f"apz-acceptance-{self.machine_class}-")
        )
        reused = work_dir_reuse_reason(self.artifacts_dir)
        if reused is not None:
            # Refused, not used: see `work_dir_reuse_reason`. A run that reuses a
            # work directory reports results that belong to a *mix* of two runs.
            raise LabError(f"the work directory {self.artifacts_dir} cannot be used afresh: {reused}")
        # A *fresh* work directory that does not exist yet is created here, not
        # by the first lab: `work_dir_reuse_reason` deliberately accepts a path
        # that does not exist, and a run whose every suite is BLOCKED for its
        # machine class never brings a lab up (`ctx.new_dir` is what creates the
        # directory otherwise) while the runner still writes this run's
        # kernel-seccomp audit into the work directory. Found 2026-10-11: an
        # explicit `--artifacts` path that did not exist made that write crash a
        # fully-blocked run (FileNotFoundError) instead of recording its BLOCKED
        # result, so the hardware-class rows were lost.
        try:
            self.artifacts_dir.mkdir(parents=True, exist_ok=True)
        except OSError as err:
            raise LabError(f"the work directory {self.artifacts_dir} cannot be created: {err}") from err
        self.stamp = time.strftime("%Y%m%dT%H%M%S")
        self.findings: list[dict] = []
        self.suites: list[Suite] = []
        self.artifacts: list[Path] = []
        self._canary: dict[str, str] | None = None
    def register_artifact(self, path: Path) -> None:
        """Record a file the runner keeps as evidence for the result row."""
        if path not in self.artifacts:
            self.artifacts.append(path)
    # -- paths -------------------------------------------------------------
    def bin(self, name: str) -> Path:
        path = self.bin_dir / name
        if not path.exists():
            raise LabError(f"missing binary {path} (build the workspace first)")
        return path
    def new_dir(self, name: str, mode: int = 0o755) -> Path:
        path = self.artifacts_dir / name
        path.mkdir(parents=True, exist_ok=True, mode=mode)
        os.chmod(path, mode)
        return path
    # -- machine gate ------------------------------------------------------
    def can_run(self, suite_name: str) -> str | None:
        """Why ``suite_name`` cannot run here, or ``None`` when it can."""
        want_os, want_arch = CLASS_PLATFORM[self.machine_class]
        if (self.host_os, self.host_arch) != (want_os, want_arch):
            return (
                f"machine class {self.machine_class!r} needs a {want_os}/{want_arch} host; "
                f"this host is {self.host_os}/{self.host_arch}"
            )
        if not Path("/dev/kvm").exists() and self.host_os == "linux" and self.machine_class != "apple":
            return "no /dev/kvm on this host"
        for name in ("noded", "vsockd"):
            if not (self.bin_dir / name).exists():
                return f"missing binary {self.bin_dir / name} (build the workspace first)"
        if not self.kernel.exists():
            return f"missing guest kernel {self.kernel}"
        if not self.initrd.exists():
            return f"missing guest initrd {self.initrd}"
        return None
    # -- manifests ---------------------------------------------------------
    def manifest(self, suite: str, **extra) -> dict:
        revision = git_revision()
        images = {
            "kernel": str(self.kernel),
            "kernel_sha256": sha256_file(self.kernel),
            "initrd": str(self.initrd),
            "initrd_sha256": sha256_file(self.initrd),
        }
        if self.workspace_image.is_file():
            # The base image of the writable `/workspace` volume: the budgets
            # suite's `--workspace` cells boot copies of it, so its digest is
            # part of the manifest exactly as the kernel's and initrd's are.
            images["workspace"] = str(self.workspace_image)
            images["workspace_bytes"] = self.workspace_image.stat().st_size
            images["workspace_sha256"] = sha256_file(self.workspace_image)
        cpus, mem = os.cpu_count(), 0
        try:
            for line in Path("/proc/meminfo").read_text().splitlines():
                if line.startswith("MemTotal:"):
                    mem = int(line.split()[1]) // 1024
        except OSError:
            pass
        return {
            "suite": suite,
            "harness_version": HARNESS_VERSION,
            "sut_git": revision,
            "vmm_version": "0.1.0 (crates/vmm Cargo.toml)",
            "vm_image": images,
            "host": {
                "machine_class": self.machine_class,
                "os": self.host_os,
                "arch": self.host_arch,
                "hostname": socket.gethostname(),
                "kernel": os.uname().release,
                "cpus": cpus,
                "memory_mib": mem,
                "rustc": _tool_version("rustc --version"),
            },
            "model_family": None,
            "model_note": (
                "no model family is configured in this repository (no provider/keys/model "
                "config exists); the MVP demo task is the frozen vsock session, so the "
                "machine x model cells collapse to the machine dimension"
            ),
            "seed": self.seed,
            "artifacts_dir": str(self.artifacts_dir),
            "started_at": iso_now(),
            "binaries": {
                name: {"path": str(self.bin_dir / name), "sha256": sha256_file(self.bin_dir / name)}
                for name in ("noded", "vsockd")
                if (self.bin_dir / name).exists()
            },
            **extra,
        }

    # -- canary ------------------------------------------------------------

    def canary_env(self) -> dict[str, str]:
        """The per-run canaries planted in both daemons' environments.

        Both are unique per invocation and one is credential-shaped, so the
        scan proves that (a) the marker never crosses into guest-reachable
        state and (b) credential-shaped host values do not either.
        """
        if self._canary is None:
            marker = hashlib.sha256(f"{self.stamp}-{os.getpid()}-{self.seed}".encode()).hexdigest()
            self._canary = {
                "APZ_CANARY": f"apz-canary-{marker[:16]}",
                "APZ_CANARY_CRED": "AKIA" + marker[:16].upper(),
            }
        return self._canary

    def planted_canaries(self) -> dict[str, str] | None:
        """The canaries a suite actually planted, or ``None`` when none did.

        Recorded in the manifest instead of generating values on the way out,
        so a run whose red-team suite did not execute never reports canaries
        that were not in any daemon's environment.
        """
        return dict(self._canary) if self._canary is not None else None

    def artifact_ref(self, path: Path, limit: int = 1 << 16) -> dict:
        """A linkable artifact reference: path, hash, size, and a bounded excerpt."""
        data = path.read_bytes() if path.exists() else b""
        return {
            "path": str(path),
            "bytes": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
            "excerpt": data[-limit:].decode(errors="replace"),
        }


def _tool_version(command: str) -> str:
    try:
        out = subprocess.run(command.split(), capture_output=True, text=True, timeout=20, check=False)
        return out.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""
