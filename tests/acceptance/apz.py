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
#: Source: PORT_STATUS.md slice 16 (owner criterion "50 microVMs < 8 GiB";
#: validated 5.57 GiB).
FLEET_RSS_BUDGET = 8 << 30

#: Steady-state CPU budget for the same fleet, as a percentage of one core.
#: Source: PORT_STATUS.md slice 16 ("low CPU"); the validated figure there is
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
        uds_dir: Path | None = None,
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
        self.noded: subprocess.Popen | None = None
        self.vsockd: subprocess.Popen | None = None
        self.port: int = 0
        self.noded_log = self.log_dir / "noded.log"
        self.vsockd_log = self.log_dir / "vsockd.log"
        self.started_at = iso_now()
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
        if self.cids:
            self.vsockd = self._spawn(
                "vsockd",
                [
                    str(self.ctx.bin("vsockd")),
                    "--uds-dir",
                    str(self.uds_dir),
                    "--cids",
                    ",".join(str(cid) for cid in self.cids),
                    "--port",
                    str(VSOCK_PORT),
                ],
                self.vsockd_log,
            )
            self._await_line(self.vsockd_log, "listening on")
            # The daemon binds its listeners one CID at a time, so wait for
            # every one of them: a probe or a UDS enumeration before the last
            # bind would see a half-started daemon.
            self._await_listeners()
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
    def _spawn(self, name: str, argv: list[str], log: Path) -> subprocess.Popen:
        handle = open(log, "ab", buffering=0)
        try:
            return subprocess.Popen(
                argv,
                stdout=handle,
                stderr=subprocess.STDOUT,
                stdin=subprocess.DEVNULL,
                start_new_session=True,
                env={**os.environ, **self.canary_env},
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
    def ping(self) -> bool:
        """Whether the daemon is still answering its API."""
        try:
            return self._request("GET", "/api/v0/machine").status == 200
        except (OSError, ValueError):
            return False
    def stream_events(self, seconds: float) -> list[str]:
        """Consume ``GET /api/v0/events`` for ``seconds`` and return its lines."""
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
                lines.append(raw.decode(errors="replace"))
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
    def note(self, text: str) -> None:
        self.notes.append(text)
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
        return self.tasks_passed >= self.required and all(item["ok"] for item in self.invariants)
    def view(self) -> dict:
        return {
            "name": self.name,
            "detail": self.detail,
            "vector": self.vector,
            "ok": self.ok,
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
        if self.blocked_reason is not None:
            return False
        return bool(self.cases) and all(case.ok for case in self.cases)
    @property
    def status(self) -> str:
        if self.blocked_reason is not None:
            return "BLOCKED"
        return "PASS" if self.ok else "FAIL"
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
        self.results_dir = Path(args.results_dir).resolve()
        self.machine_class = args.machine_class
        self.host_os, self.host_arch = host_platform()
        self.keep = bool(args.keep)
        self.seed = args.seed
        self.runner_timeout_scale = float(args.timeout_scale)
        self.artifacts_dir = Path(
            args.artifacts or tempfile.mkdtemp(prefix=f"apz-acceptance-{self.machine_class}-")
        )
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
