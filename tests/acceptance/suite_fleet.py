"""Suite 5 — the agent fleet: keepalive sessions, the hub, and the reap contract.

The fleet contract (``docs/agents/fleet.md``) is additive over the frozen v1
demo session: a ``hello`` carrying ``"keepalive": true`` opens a long-lived
session the hub holds open — heartbeats in, the hub's own tool calls out, a
``stop`` the agent answers with ``bye`` to end it — backed by an in-memory
per-CID registry the hub logs and, when the build carries the status surface,
publishes as JSON (`GET /status`).

This suite proves that contract end to end against the real ``vsockd``, driven
by stub agents that speak the same newline-delimited JSON the guest harness
speaks (the parameters below are the suite's own; the demo matrix's frozen v1
path is untouched). Seven contracts, several of them conditional on a sibling
having landed:

1. the keepalive lifecycle — a role welcome, rate-limited heartbeats, a
   guest-initiated tool call the hub answers, a clean end, and the registry row
   draining;
2. a hub restart — the daemon dies ungracefully leaving its listener files, the
   restart clears them and rebinds, the agent re-registers, and v1 traffic after
   the restart is byte-identical to v1;
3. re-adoption — a same-CID reconnect replaces a live row (the contract's
   ``re-adopted`` line), and the predecessor's disconnect cannot evict it;
4. the reap/hangup contract — an agent silent inside the 300 s deadline is still
   alive and served, and every disconnect path drains its row;
5. the read-only status surface (``vsockd --status-port``), when it is built;
6. the real keepalive fleet (``noded --agents``), when that flag is built — two
   role guests that connect, beat, stay up across a hub restart, and tear down
   cleanly;
7. the hub through ``noded``'s one-origin proxy (``GET /api/v0/hub``), when
   ``noded --hub-status`` is built — the hub's JSON verbatim, 502 when the hub
   is unreachable, 503 when the daemon was not told where it is.

A case whose flag is not in the build is reported BLOCKED with the reason —
never silently skipped, never counted as a pass.
"""

from __future__ import annotations

import json
import os
import re
import signal
import socket
import subprocess
import time
from pathlib import Path

from apz import (
    VSOCK_PORT,
    Lab,
    Suite,
    now,
    probe_unix_socket,
    reconstruct_console,
    work_dir_reuse_reason,
)

#: The orchestrator's handshake echo (the frozen protocol).
WELCOME_ECHO = "plaza-orchestrator"

#: The CIDs this suite serves, one role each. They are disjoint from every
#: other suite's labs, so a stray probe can never be mistaken for a real one.
CID_LIFECYCLE = 50
CID_RESTART = 51
CID_V1 = 52
CID_READOPT = 53
CID_REAP = 54
CID_STATUS = 55
CID_PROXY = 56

#: The role fleet's CIDs (``noded --agents-cid-base`` defaults to 100).
AGENT_ROLES = ("builder", "reviewer")
AGENT_CID_BASE = 100

#: The hub's own constants, from ``crates/vsockd/src/main.rs``. The suite
#: asserts ``/status`` reports them rather than inventing any.
KEEPALIVE_IDLE_SECS = 300
MAX_SESSIONS = 64

#: The hub's heartbeat log rate: the 1st beat and then every ``N``th
#: (``crates/vsockd/src/hub.rs`` ``HEARTBEAT_LOG_EVERY``). Twelve beats must
#: therefore produce exactly two log lines (the 1st and the 12th).
HEARTBEAT_LOG_EVERY = 12

#: How long a keepalive agent is left silent and must still be alive: the hub's
#: reap deadline is 300 s, so a silence far shorter than it must never reap.
SILENCE_WINDOW_S = 10.0

#: How long a log transition, a registry change, or a guest transcript is given.
SETTLE_S = 15.0

#: How long a real role guest is given to boot, greet the hub and beat.
GUEST_SETTLE_S = 60.0

#: How long a teardown has to unlink its muxer sockets.
SOCKET_SETTLE_S = 5.0


# ------------------------------------------------------------ probe the build


def _flag(ctx, binary: str, needle: str) -> bool:
    """Whether ``binary``'s CLI advertises ``needle`` (a ``--help`` probe).

    A sibling slice's flag can be absent from the build entirely; the suite
    gates the assertions that need it behind this probe rather than assuming it
    and failing on its absence.
    """
    try:
        out = subprocess.run(
            [str(ctx.bin(binary)), "--help"], capture_output=True, text=True, timeout=20, check=False
        )
    except (OSError, subprocess.SubprocessError):
        return False
    return needle in out.stdout


# ------------------------------------------------------------------- the stub


class Stub:
    """A stub keepalive agent: raw JSON lines over the hub's UDS listener.

    The acceptance suite is a client of the hub, not a guest: it opens the
    daemon's per-CID Unix socket (``{uds}/vm{cid}_{port}`` — the path the VMM
    muxer dials for that guest) and speaks exactly the v2 lines the harness
    speaks, so the hub's own serve path is exercised without booting a VM.
    """

    def __init__(self, path: Path, cid: int, role: str | None, *, keepalive: bool = True, timeout: float = 10.0):
        self.cid = cid
        self.role = role
        self.keepalive = keepalive
        self.timeout = timeout
        self.buffer = b""
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(timeout)
        self.sock.connect(str(path))

    def send(self, message: dict) -> None:
        self.sock.sendall((json.dumps(message) + "\n").encode())

    def recv(self, timeout: float | None = None) -> dict:
        """The next JSON line (``TimeoutError``/``ConnectionError`` otherwise)."""
        deadline = now() + (self.timeout if timeout is None else timeout)
        while True:
            if b"\n" in self.buffer:
                line, self.buffer = self.buffer.split(b"\n", 1)
                return json.loads(line.decode())
            remaining = deadline - now()
            if remaining <= 0:
                raise TimeoutError(f"no line within the window (buffer={self.buffer!r})")
            self.sock.settimeout(remaining)
            try:
                chunk = self.sock.recv(1 << 16)
            except (TimeoutError, socket.timeout):
                raise TimeoutError(f"no line within the window (buffer={self.buffer!r})")
            except OSError as err:
                raise ConnectionError(str(err))
            if not chunk:
                raise ConnectionError("the hub closed the connection")
            self.buffer += chunk

    def hello(self) -> dict:
        """Offer the handshake and return the ``welcome`` (or raise)."""
        message = {"type": "hello", "vm": 0, "guest_cid": self.cid}
        if self.role is not None:
            message["role"] = self.role
        if self.keepalive:
            message["keepalive"] = True
        self.send(message)
        return self.recv()

    def await_closed(self, timeout: float = 5.0) -> bool:
        """True once the hub has closed the session (EOF), a stray line aside."""
        deadline = now() + timeout
        while now() < deadline:
            if self.buffer:
                return False
            self.sock.settimeout(min(0.25, max(0.05, deadline - now())))
            try:
                chunk = self.sock.recv(1 << 16)
            except (TimeoutError, socket.timeout):
                continue
            except OSError:
                return True
            if not chunk:
                return True
            self.buffer += chunk
            return False
        return False

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass


# --------------------------------------------------------------- small helpers


def _guarded(case, body) -> None:
    """Run a case body; a harness exception fails the case, never the suite."""
    try:
        body()
    except Exception as err:  # a case must fail, not abort the run
        case.invariant("the case completed without a harness exception", False, f"{type(err).__name__}: {err}")


def _numeric_pair(text: str) -> bool:
    fields = text.split()
    if len(fields) < 2:
        return False
    try:
        float(fields[0]), float(fields[1])
    except ValueError:
        return False
    return True


def _sleeper(predicate, timeout: float) -> bool:
    """Poll ``predicate`` until it is true or ``timeout`` elapses."""
    deadline = now() + timeout
    while now() < deadline:
        if predicate():
            return True
        time.sleep(0.05)
    return predicate()


def _log_has(lab: Lab, needle: str, timeout: float = SETTLE_S) -> bool:
    return _sleeper(lambda: needle in lab.vsockd_log_text(), timeout)


def _log_lines(lab: Lab, needle: str) -> list[str]:
    return [line for line in lab.vsockd_log_text().splitlines() if needle in line]


def _log_tail(lab: Lab, limit: int = 400) -> str:
    return lab.vsockd_log_text()[-limit:]


def _snippet(text: str, limit: int = 160) -> str:
    """The last ``limit`` characters of a console stream (a failure detail)."""
    return text[-limit:]


def _closed_port() -> int:
    """A loopback TCP port that was just bound and released (an unreachable hub)."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def _row_for(lab: Lab, cid: int) -> dict | None:
    """The ``/status`` registry row for ``cid``, or ``None``."""
    for row in lab.status().get("agents", []):
        if row.get("cid") == cid:
            return row
    return None


def _await_row(lab: Lab, cid: int, timeout: float = SETTLE_S) -> dict | None:
    _sleeper(lambda: _row_for(lab, cid) is not None, timeout)
    return _row_for(lab, cid)


def _await_no_rows(lab: Lab, timeout: float = SETTLE_S) -> bool:
    """The registry is empty and no session is active."""
    def drained() -> bool:
        body = lab.status()
        return not body.get("agents") and body.get("sessions_active") == 0
    return _sleeper(drained, timeout)


def _listeners_live(lab: Lab, cids: list[int], timeout: float = 10.0) -> tuple[bool, dict]:
    """Wait until every served CID's listener actually answers a connection."""
    def all_live() -> bool:
        return all(probe_unix_socket(lab.listener_marker(cid)) == "answers" for cid in cids)
    ok = _sleeper(all_live, timeout)
    return ok, {cid: probe_unix_socket(lab.listener_marker(cid)) for cid in cids}


def _await_muxer_gone(lab: Lab, cids: list[int], settle: float = SOCKET_SETTLE_S) -> list[str]:
    wanted = {f"vm{cid}" for cid in cids}
    _sleeper(lambda: not (wanted & set(lab.uds_entries())), settle)
    return sorted(wanted & set(lab.uds_entries()))


def _v1_session(lab: Lab, cid: int) -> dict:
    """Drive one frozen v1 session and return what happened on the wire."""
    stub = Stub(lab.listener_marker(cid), cid, None, keepalive=False)
    try:
        welcome = stub.hello()
        stub.send({"type": "tool_call", "call": 1, "name": "echo", "args": {"text": f"plaza-guest-{cid}-hello"}})
        echo = stub.recv()
        stub.send({"type": "tool_call", "call": 2, "name": "uptime", "args": {}})
        uptime = stub.recv()
        stub.send({"type": "bye", "calls": 2})
        closed = stub.await_closed()
    finally:
        stub.close()
    return {"welcome": welcome, "echo": echo, "uptime": uptime, "closed": closed}


# ------------------------------------------------------------------- the cases


def _case_lifecycle(suite: Suite, ctx, status: bool) -> None:
    case = suite.case(
        "keepalive lifecycle: role welcome, heartbeats, tool call/answer, clean end",
        vector=(
            "stub agent role=architect keepalive: hello -> welcome(role,keepalive) -> 12 heartbeats "
            "(the hub's rate-limited line) -> guest tool_call -> hub tool_result -> bye -> the row drains"
        ),
    )

    def body() -> None:
        cid = CID_LIFECYCLE
        with Lab(ctx, [cid], "fleet-lifecycle", status_port=0 if status else None) as lab:
            stub = Stub(lab.listener_marker(cid), cid, "architect")
            try:
                welcome = stub.hello()
                case.invariant(
                    "welcome echoes the accepted role and the keepalive flag",
                    welcome.get("type") == "welcome"
                    and welcome.get("echo") == WELCOME_ECHO
                    and welcome.get("role") == "architect"
                    and welcome.get("keepalive") is True,
                    f"welcome={welcome}",
                )
                case.invariant(
                    "the hub logged the connect transition with the role",
                    _log_has(lab, f"vsockd: agent {cid} role architect connected"),
                    f"welcome={welcome}",
                )

                # Twelve heartbeats: the hub logs the 1st and every 12th, so
                # exactly two lines (its own count, not the guest's seq, drives
                # the limiter). The registry's `last_heartbeat` is sampled with a
                # deliberate pause before the remaining beats: the timestamps
                # are millisecond-resolution, so sampling across a
                # sub-millisecond burst would compare two equal values.
                first_beat = None
                stub.send({"type": "heartbeat", "seq": 1})
                if status:
                    first_beat = (_row_for(lab, cid) or {}).get("last_heartbeat_ms")
                    # Wider than a millisecond, so the later beats land strictly
                    # after this sample rather than tying with it.
                    time.sleep(0.05)
                for seq in range(2, HEARTBEAT_LOG_EVERY + 1):
                    stub.send({"type": "heartbeat", "seq": seq})
                expected_lines = [
                    f"vsockd: agent {cid} heartbeat 1",
                    f"vsockd: agent {cid} heartbeat {HEARTBEAT_LOG_EVERY}",
                ]
                case.invariant(
                    "the hub logged exactly the rate-limited heartbeat lines (1st and 12th)",
                    _sleeper(lambda: _log_lines(lab, f"agent {cid} heartbeat") == expected_lines, SETTLE_S),
                    f"lines={_log_lines(lab, f'agent {cid} heartbeat')}",
                )

                # A guest-initiated tool call, answered by the hub as in v1.
                stub.send({"type": "tool_call", "call": 1, "name": "echo", "args": {"text": f"plaza-agent-{cid}"}})
                echo = stub.recv()
                case.invariant(
                    "the hub answered the guest's echo tool call",
                    echo.get("type") == "tool_result"
                    and echo.get("call") == 1
                    and echo.get("ok") is True
                    and echo.get("output") == f"plaza-agent-{cid}",
                    f"result={echo}",
                )
                case.invariant(
                    "the hub logged the answered guest call",
                    _log_has(lab, f"vsockd: answered call 1 (echo) for agent {cid}"),
                    _log_tail(lab),
                )
                stub.send({"type": "tool_call", "call": 2, "name": "uptime", "args": {}})
                uptime = stub.recv()
                case.invariant(
                    "the hub answered the guest's uptime tool call",
                    uptime.get("type") == "tool_result"
                    and uptime.get("call") == 2
                    and uptime.get("ok") is True
                    and _numeric_pair(str(uptime.get("output", ""))),
                    f"result={uptime}",
                )

                if status:
                    row = _await_row(lab, cid)
                    case.invariant(
                        "the registry row carries the cid, the accepted role and the call count",
                        row is not None
                        and row.get("role") == "architect"
                        and row.get("cid") == cid
                        and row.get("calls") == 2,
                        f"row={row}",
                    )
                    case.invariant(
                        "last_heartbeat advanced across the beats",
                        row is not None and first_beat is not None and row.get("last_heartbeat_ms", 0) > first_beat,
                        f"first={first_beat} last={(row or {}).get('last_heartbeat_ms')}",
                    )
                else:
                    case.note("registry assertions SKIPPED: vsockd --status-port is not in this build")

                # The clean end: the agent answers with bye; the hub ends the
                # session and drains the row.
                stub.send({"type": "bye", "calls": 2})
                case.invariant(
                    "the hub logged a clean end after 2 tool calls",
                    _log_has(lab, f"vsockd: agent {cid} session ended cleanly after 2 tool calls"),
                    _log_tail(lab),
                )
                case.invariant("the hub closed the session after the bye", stub.await_closed())
                if status:
                    case.invariant("the registry row drained after the bye", _await_no_rows(lab), f"status={lab.status()}")
            finally:
                stub.close()

    _guarded(case, body)
    case.note(
        "direction covered: the guest-initiated tool_call -> hub tool_result (and the guest's bye). "
        "The hub-initiated tool_call/stop direction has no producer in this build (the session's "
        "steering sender, Hub::steer, is #[cfg(test)]; nothing in the daemon fills it this slice), so "
        "it is exercised by the crate's own UDS test and is not drivable from an acceptance stub."
    )


def _case_hub_restart(suite: Suite, ctx, status: bool) -> None:
    case = suite.case(
        "reconnect across a hub restart: clean rebind over stale listeners, re-registration, v1 unaffected",
        vector=(
            "stub keepalive builder -> SIGKILL vsockd (its listener files are left behind) -> restart on "
            "the SAME uds dir and CIDs -> re-hello registers afresh -> one v1 session after the restart"
        ),
    )

    def body() -> None:
        first = Lab(ctx, [CID_RESTART, CID_V1], "fleet-restart-a", status_port=0 if status else None)
        uds = first.uds_dir
        first_connected_ms: int | None = None
        stub = None
        try:
            first.start()
            stub = Stub(first.listener_marker(CID_RESTART), CID_RESTART, "builder")
            welcome = stub.hello()
            case.invariant(
                "the first session is a keepalive builder session",
                welcome.get("role") == "builder" and welcome.get("keepalive") is True,
                f"welcome={welcome}",
            )
            stub.send({"type": "heartbeat", "seq": 1})
            _log_has(first, f"vsockd: agent {CID_RESTART} heartbeat 1")
            if status:
                row = _await_row(first, CID_RESTART)
                first_connected_ms = (row or {}).get("connected_at_ms")
            # The daemon dies ungracefully: unlike SIGTERM it runs no cleanup,
            # so its listener socket files stay on disk — the stale path a
            # restart has to clear and rebind over.
            os.kill(first.vsockd.pid, signal.SIGKILL)
            first.vsockd.wait(timeout=10)
            case.invariant("the daemon died ungracefully (SIGKILL)", first.vsockd.poll() is not None)
            stale_verdict = probe_unix_socket(first.listener_marker(CID_RESTART))
            case.invariant(
                "the ungraceful death left a stale listener file behind (the path a restart must clear)",
                stale_verdict == "refused",
                f"probe={stale_verdict}",
            )
            case.invariant("the agent's connection dropped with the daemon", stub.await_closed())
        finally:
            if stub is not None:
                stub.close()
            first.stop()

        second = Lab(ctx, [CID_RESTART, CID_V1], "fleet-restart-b", uds_dir=uds, status_port=0 if status else None)
        try:
            second.start()
            live, verdicts = _listeners_live(second, [CID_RESTART, CID_V1])
            case.invariant("the restarted hub rebound over the stale listener files", live, f"probes={verdicts}")
            stub2 = Stub(second.listener_marker(CID_RESTART), CID_RESTART, "builder")
            try:
                welcome2 = stub2.hello()
                case.invariant(
                    "the agent reconnects and is welcomed again",
                    welcome2.get("role") == "builder" and welcome2.get("keepalive") is True,
                    f"welcome={welcome2}",
                )
                case.invariant(
                    "the restarted hub logged the agent's registration",
                    _log_has(second, f"vsockd: agent {CID_RESTART} role builder connected"),
                    _log_tail(second),
                )
                if status:
                    row = _await_row(second, CID_RESTART)
                    fresh = (row or {}).get("connected_at_ms")
                    case.invariant(
                        "the registry shows the agent again with a fresh connected_at",
                        row is not None
                        and fresh is not None
                        and (first_connected_ms is None or fresh >= first_connected_ms),
                        f"before={first_connected_ms} after={fresh}",
                    )
                else:
                    case.note("registry assertions SKIPPED: vsockd --status-port is not in this build")
                stub2.send({"type": "tool_call", "call": 1, "name": "echo", "args": {"text": "after-restart"}})
                result = stub2.recv()
                case.invariant(
                    "the re-registered session serves a tool call",
                    result.get("call") == 1 and result.get("ok") is True and result.get("output") == "after-restart",
                    f"result={result}",
                )
                stub2.send({"type": "bye", "calls": 1})
                case.invariant(
                    "the re-registered session ends cleanly",
                    _log_has(second, f"vsockd: agent {CID_RESTART} session ended cleanly after 1 tool calls"),
                    _log_tail(second),
                )
            finally:
                stub2.close()

            # v1 traffic after the restart: byte-identical to the frozen welcome.
            v1 = _v1_session(second, CID_V1)
            case.invariant(
                "a v1 session after the restart gets the frozen welcome byte-shape",
                v1["welcome"] == {"type": "welcome", "echo": WELCOME_ECHO},
                f"welcome={v1['welcome']}",
            )
            case.invariant(
                "the v1 session's tool calls are answered end to end",
                v1["echo"].get("ok") is True
                and v1["echo"].get("output") == f"plaza-guest-{CID_V1}-hello"
                and v1["uptime"].get("ok") is True
                and _numeric_pair(str(v1["uptime"].get("output", ""))),
                f"echo={v1['echo']} uptime={v1['uptime']}",
            )
            case.invariant(
                "the v1 session ends cleanly and the hub logs it",
                v1["closed"]
                and _log_has(second, f"vsockd: session with guest {CID_V1} vm 0 ended cleanly after 2 tool calls"),
                f"closed={v1['closed']}",
            )
        finally:
            second.stop()

    _guarded(case, body)
    case.note(
        "the contract's `re-adopted` line fires when a reconnect replaces a LIVE row; a restarted hub "
        "has an empty in-memory registry, so a post-restart reconnect registers afresh (asserted here). "
        "Live-row replacement is proven deterministically by the re-adoption case."
    )


def _case_readoption(suite: Suite, ctx, status: bool) -> None:
    case = suite.case(
        "re-adoption: a same-CID reconnect replaces a live row (the contract's re-adopted line)",
        vector=(
            "two concurrent sessions on one CID: the second re-adopts the live row (logged), and the "
            "predecessor's disconnect cannot evict its successor's row"
        ),
    )

    def body() -> None:
        cid = CID_READOPT
        with Lab(ctx, [cid], "fleet-readopt", status_port=0 if status else None) as lab:
            first = Stub(lab.listener_marker(cid), cid, "builder")
            second = Stub(lab.listener_marker(cid), cid, "builder")
            try:
                welcome_first = first.hello()
                first.send({"type": "heartbeat", "seq": 1})
                _log_has(lab, f"vsockd: agent {cid} role builder connected")
                # The second connection arrives while the first row is still live.
                welcome_second = second.hello()
                case.invariant(
                    "both sessions are welcomed as keepalive builders",
                    welcome_first.get("role") == "builder" and welcome_second.get("role") == "builder",
                    f"first={welcome_first} second={welcome_second}",
                )
                case.invariant(
                    "the hub logged the re-adoption of the live row",
                    _log_has(lab, f"vsockd: agent {cid} re-adopted (previous session role builder"),
                    _log_tail(lab, 600),
                )
                # The predecessor leaves; its stale token must not evict the row.
                first.close()
                _log_has(lab, "connection lost (the keepalive agent's connection dropped")
                second.send({"type": "tool_call", "call": 1, "name": "echo", "args": {"text": "still-mine"}})
                result = second.recv()
                case.invariant(
                    "the adopted session is still served after the predecessor leaves",
                    result.get("call") == 1 and result.get("output") == "still-mine",
                    f"result={result}",
                )
                if status:
                    row = _row_for(lab, cid)
                    case.invariant(
                        "the registry still carries the adopted row (the predecessor could not evict it)",
                        row is not None and row.get("role") == "builder",
                        f"row={row} log={_log_tail(lab, 600)}",
                    )
                else:
                    case.note("registry assertions SKIPPED: vsockd --status-port is not in this build")
                second.send({"type": "bye", "calls": 1})
                case.invariant(
                    "the adopted session ends cleanly",
                    _log_has(lab, f"vsockd: agent {cid} session ended cleanly after 1 tool calls"),
                    _log_tail(lab),
                )
            finally:
                first.close()
                second.close()

    _guarded(case, body)


def _case_reap_and_hangup(suite: Suite, ctx, status: bool) -> None:
    case = suite.case(
        "reap/hangup contract: no premature reap, a silent agent stays alive, every row drains",
        vector=(
            f"keepalive reviewer silent {SILENCE_WINDOW_S:.0f}s (the deadline is {KEEPALIVE_IDLE_SECS}s) then "
            "still served; a disconnect without bye drains the row; 3 further connect/drop cycles leave 0 rows"
        ),
    )

    def body() -> None:
        cid = CID_REAP
        with Lab(ctx, [cid], "fleet-reap", status_port=0 if status else None) as lab:
            stub = Stub(lab.listener_marker(cid), cid, "reviewer")
            try:
                welcome = stub.hello()
                case.invariant("the agent is welcomed as a keepalive reviewer", welcome.get("role") == "reviewer")
                for seq in (1, 2, 3):
                    stub.send({"type": "heartbeat", "seq": seq})
                _log_has(lab, f"vsockd: agent {cid} heartbeat 1")

                # Silence: the deadline is 300 s, so a bounded silence must not reap.
                time.sleep(SILENCE_WINDOW_S)
                case.invariant(
                    "no premature reap inside the observation window",
                    "reaped idle" not in lab.vsockd_log_text(),
                    _log_tail(lab),
                )
                if status:
                    row = _row_for(lab, cid)
                    case.invariant("the registry row persists across the silence", row is not None, f"row={row}")
                else:
                    case.note("registry assertions SKIPPED: vsockd --status-port is not in this build")

                # Still alive: a fresh tool call is answered.
                stub.send({"type": "tool_call", "call": 1, "name": "echo", "args": {"text": "alive-after-silence"}})
                result = stub.recv()
                case.invariant(
                    "the silent agent is still served (it was not reaped)",
                    result.get("call") == 1 and result.get("output") == "alive-after-silence",
                    f"result={result}",
                )
            finally:
                # Disconnect without a bye: the hub reports the drop as a loss
                # and its RAII registration drains the row.
                stub.close()

            case.invariant(
                "the hub reported the dropped connection as a loss, not a protocol violation",
                _log_has(lab, "connection lost (the keepalive agent's connection dropped"),
                _log_tail(lab),
            )
            if status:
                case.invariant("the dropped session's row drained", _await_no_rows(lab), f"status={lab.status()}")
            else:
                case.note("registry assertions SKIPPED: vsockd --status-port is not in this build")

            # Three more connect/drop cycles: no row (and so no session slot) leaks.
            accepted = 0
            for _cycle in range(3):
                cycle = Stub(lab.listener_marker(cid), cid, "reviewer")
                try:
                    if cycle.hello().get("role") == "reviewer":
                        accepted += 1
                    cycle.send({"type": "heartbeat", "seq": 1})
                    time.sleep(0.2)
                finally:
                    cycle.close()
            case.invariant("every churn connection was accepted", accepted == 3, f"accepted={accepted}")
            if status:
                case.invariant(
                    "3 connect/drop cycles leave the registry at 0 (no row leak)",
                    _await_no_rows(lab),
                    f"status={lab.status()}",
                )
            case.invariant(
                "no reap was ever logged (nothing went silent past the deadline)",
                "reaped idle" not in lab.vsockd_log_text(),
                _log_tail(lab),
            )

    _guarded(case, body)
    case.note(
        f"the {KEEPALIVE_IDLE_SECS}s reap deadline itself is unit-tested in the crate with an injected "
        "clock (a real 300 s wait has no place in an acceptance run); this case asserts the observable "
        "half — no reap inside a bounded silence, and a drained registry on every disconnect path."
    )


def _case_status_surface(suite: Suite, ctx) -> None:
    case = suite.case(
        "the read-only status surface reports the hub's registry",
        vector="GET /status (node facts + agent rows) and GET /healthz against a live agent",
    )
    if not _flag(ctx, "vsockd", "--status-port"):
        case.blocked(
            "vsockd --status-port is not in this build's CLI (probe: vsockd --help); the status "
            "surface has not landed, so the registry assertions are SKIPPED"
        )
        return

    def body() -> None:
        cid = CID_STATUS
        with Lab(ctx, [cid], "fleet-status", status_port=0) as lab:
            body_json = lab.status()
            node = body_json.get("node", {})
            case.invariant(
                "the node facts carry the served CIDs, the port and the frozen limits",
                node.get("cids") == [cid]
                and node.get("port") == VSOCK_PORT
                and node.get("max_sessions") == MAX_SESSIONS
                and node.get("keepalive_idle_s") == KEEPALIVE_IDLE_SECS
                and isinstance(node.get("uptime_s"), int),
                f"node={node}",
            )
            case.invariant(
                "the registry is empty before any agent connects",
                body_json.get("agents") == [] and body_json.get("sessions_active") == 0,
                f"body={body_json}",
            )
            health = lab.healthz()
            case.invariant(
                "GET /healthz answers ok", health.status == 200 and "ok" in health.body, f"{health.status} {health.body!r}"
            )
            missing = lab.status_get("/no-such-path")
            case.invariant("an unknown path is a 404", missing.status == 404, f"status={missing.status}")

            stub = Stub(lab.listener_marker(cid), cid, "architect")
            try:
                stub.hello()
                stub.send({"type": "heartbeat", "seq": 1})
                row = _await_row(lab, cid)
                case.invariant(
                    "a connected agent appears as a row with cid/role/connected_at/last_heartbeat/calls",
                    row is not None
                    and row.get("cid") == cid
                    and row.get("role") == "architect"
                    and isinstance(row.get("connected_at_ms"), int)
                    and isinstance(row.get("last_heartbeat_ms"), int)
                    and row.get("calls") == 0,
                    f"row={row}",
                )
                case.invariant(
                    "sessions_active is 1 while the agent is connected",
                    lab.status().get("sessions_active") == 1,
                )
                stub.send({"type": "bye", "calls": 0})
                case.invariant("the surface drains after a clean end", _await_no_rows(lab), f"status={lab.status()}")
            finally:
                stub.close()

    _guarded(case, body)
    case.note("the surface is read-only by contract; a mutating method is answered 405 and is not exercised here")


def _case_hub_proxy(suite: Suite, ctx, hub_ok: bool) -> None:
    case = suite.case(
        "the hub's status surface through noded's one-origin proxy",
        vector=(
            "GET /api/v0/hub[/healthz] proxies vsockd's /status[/healthz] verbatim; 502 when the hub is "
            "unreachable, 503 when noded was not told where it is"
        ),
    )
    if not hub_ok:
        case.blocked(
            "noded --hub-status is not in this build's CLI (probe: noded --help); the one-origin hub "
            "proxy has not landed, so the case is SKIPPED"
        )
        return

    def body() -> None:
        cid = CID_PROXY
        # 200: the proxy points at this lab's own hub, and the bodies agree
        # byte for byte. `uptime_s` is second-granular, so a pair that straddles
        # a tick is retried rather than compared across a boundary.
        with Lab(ctx, [cid], "fleet-proxy", status_port=0, hub_status="self") as lab:
            matched = False
            for _attempt in range(5):
                proxied = lab.api_get("/api/v0/hub")
                if proxied.status == 200 and proxied.body == lab.status_get("/status").body:
                    matched = True
                    break
            case.invariant(
                "GET /api/v0/hub is vsockd's /status byte-for-byte",
                matched,
                f"proxied={lab.api_get('/api/v0/hub').body[:200]!r} direct={lab.status_get('/status').body[:200]!r}",
            )
            proxy_health = lab.api_get("/api/v0/hub/healthz")
            case.invariant(
                "GET /api/v0/hub/healthz is the hub's /healthz byte-for-byte",
                proxy_health.status == 200 and proxy_health.body == lab.status_get("/healthz").body,
                f"proxied={proxy_health.status} {proxy_health.body!r}",
            )
            stub = Stub(lab.listener_marker(cid), cid, "architect")
            try:
                stub.hello()
                stub.send({"type": "heartbeat", "seq": 1})
                _await_row(lab, cid)
                live = lab.api_get("/api/v0/hub").json
                case.invariant(
                    "the proxied view is live: a connected agent appears in it",
                    [row.get("cid") for row in live.get("agents", [])] == [cid],
                    f"agents={live.get('agents')}",
                )
            finally:
                stub.close()

        # 503: noded was not told where the hub is.
        with Lab(ctx, [CID_PROXY], "fleet-proxy-off", hub_status="off") as lab:
            case.invariant(
                "a noded not told where the hub is answers 503",
                lab.api_get("/api/v0/hub").status == 503,
                f"status={lab.api_get('/api/v0/hub').status}",
            )
        # 502: a hub that is not there.
        with Lab(ctx, [CID_PROXY], "fleet-proxy-down", hub_status=f"127.0.0.1:{_closed_port()}") as lab:
            case.invariant(
                "a noded whose hub is unreachable answers 502",
                lab.api_get("/api/v0/hub").status == 502,
                f"status={lab.api_get('/api/v0/hub').status}",
            )

    _guarded(case, body)


def _write_manifest(ctx) -> Path:
    """A 2-role manifest in the suite's own work dir (not the 21-role fleet)."""
    roles = {
        role: {
            "role": role,
            "title": f"The {role}",
            "permits": [],
            "toolsets": [],
            "budget": 1,
            "variant": None,
        }
        for role in AGENT_ROLES
    }
    path = ctx.new_dir("fleet-manifest") / "roles.json"
    path.write_text(json.dumps({"source": "tests/acceptance/suite_fleet.py", "roles": roles}, indent=2) + "\n")
    ctx.register_artifact(path)
    return path


def _case_real_guest(suite: Suite, ctx, status: bool) -> None:
    case = suite.case(
        "the real keepalive fleet: noded --agents boots role guests that connect, beat, stay up, and survive a hub restart",
        vector=(
            "noded --agents <2-role manifest>: 2 long-lived keepalive guests; the register carries their "
            "roles, the hub registers both, the guest transcript shows the accepted role + heartbeats and "
            "no reboot; SIGKILL the hub and both guests reconnect; DELETE tears the fleet down cleanly"
        ),
    )
    if not _flag(ctx, "noded", "--agents"):
        case.blocked(
            "noded --agents is not in this build's CLI (probe: noded --help); the role fleet has not "
            "landed, so the real-guest case is SKIPPED"
        )
        return

    def body() -> None:
        manifest = _write_manifest(ctx)
        cids = [AGENT_CID_BASE + index for index in range(len(AGENT_ROLES))]
        with Lab(ctx, cids, "fleet-agents", agents=manifest, status_port=0 if status else None) as lab:
            line = next(
                (candidate for candidate in lab.noded_log_text().splitlines() if "noded: initial role fleet" in candidate),
                "",
            )
            case.invariant("noded booted the initial role fleet at startup", bool(line), lab.noded_log_text()[-400:])
            match = re.search(r"noded: initial role fleet (\S+):", line)
            run_id = match.group(1) if match else None
            case.invariant("the startup line names the fleet run", run_id is not None, line)

            run = lab.get(run_id) if run_id else {}
            case.invariant(
                "the fleet run is running (long-lived; beats unset)",
                run.get("state") == "running",
                f"state={run.get('state')}",
            )
            case.invariant(
                "both VM rows carry their role",
                sorted(vm.get("role") for vm in run.get("vms", [])) == sorted(AGENT_ROLES),
                f"roles={[vm.get('role') for vm in run.get('vms', [])]}",
            )

            # Both real guests reach the hub: one connect line per CID/role.
            connected = _sleeper(
                lambda: all(
                    any(
                        f"vsockd: agent {AGENT_CID_BASE + i} role {role} connected" in candidate
                        for candidate in lab.vsockd_log_text().splitlines()
                    )
                    for i, role in enumerate(AGENT_ROLES)
                ),
                GUEST_SETTLE_S,
            )
            case.invariant("both role guests connected to the hub", connected, f"connected={_log_lines(lab, 'connected')}")
            if status:
                both = _sleeper(
                    lambda: sorted(row.get("role") for row in lab.status().get("agents", [])) == sorted(AGENT_ROLES),
                    GUEST_SETTLE_S,
                )
                case.invariant(
                    "the hub's registry shows both agents, sessions_active == 2",
                    both and lab.status().get("sessions_active") == 2,
                    f"status={lab.status()}",
                )
            else:
                case.note("registry assertions SKIPPED: vsockd --status-port is not in this build")

            # The guest's own transcript, the other side of the wire: the
            # accepted role, at least one beat, and no reboot (a keepalive guest
            # stays up; the init never reboots it).
            def transcript(with_beat: bool) -> dict:
                view = lab.get(run_id) if run_id else {}
                streams = {vm.get("role"): reconstruct_console(vm.get("serial", "")) for vm in view.get("vms", [])}
                ready = all(
                    f"agent-harness: role {role} accepted" in streams.get(role, "")
                    and (not with_beat or "agent-harness: heartbeat" in streams.get(role, ""))
                    for role in AGENT_ROLES
                )
                return {"ready": ready, "streams": streams, "state": view.get("state")}

            greeted = _sleeper(lambda: transcript(False)["ready"], GUEST_SETTLE_S)
            streams = transcript(False)["streams"]
            case.invariant(
                "each guest's transcript shows its role accepted",
                greeted,
                f"streams={ {role: _snippet(streams.get(role, '')) for role in AGENT_ROLES} }",
            )
            beated = _sleeper(lambda: transcript(True)["ready"], GUEST_SETTLE_S)
            streams = transcript(True)["streams"]
            case.invariant(
                "each guest beats its heartbeat on the console",
                beated,
                f"beat_seen={ {role: 'agent-harness: heartbeat' in streams.get(role, '') for role in AGENT_ROLES} }",
            )
            case.invariant(
                "no guest rebooted (a keepalive guest stays up)",
                all("rebooting" not in stream for stream in streams.values()),
                f"streams={ {role: _snippet(stream) for role, stream in streams.items()} }",
            )
            case.invariant(
                "the fleet is still running after the guests greeted and beat",
                (lab.get(run_id) if run_id else {}).get("state") == "running",
            )

            # A hub restart mid-fleet. `noded` (and the VMs it owns) is
            # untouched; only the hub dies. Each guest's connection drops and
            # its harness reconnects with backoff — the contract's "a hub
            # restart does not kill an agent, it re-adopts it" — so the
            # restarted hub registers both guests again.
            os.kill(lab.vsockd.pid, signal.SIGKILL)
            lab.vsockd.wait(timeout=10)
            hub = Lab(ctx, cids, "fleet-agents-hub2", uds_dir=lab.uds_dir, status_port=0 if status else None)
            try:
                hub.start()
                reconnected = _sleeper(
                    lambda: all(
                        f"vsockd: agent {AGENT_CID_BASE + index} role {role} connected" in hub.vsockd_log_text()
                        for index, role in enumerate(AGENT_ROLES)
                    ),
                    GUEST_SETTLE_S,
                )
                case.invariant(
                    "both real guests reconnect to the restarted hub",
                    reconnected,
                    f"hub_log={_log_lines(hub, 'connected')}",
                )
                if status:
                    case.invariant(
                        "the restarted hub's registry shows both agents again",
                        _sleeper(
                            lambda: sorted(row.get("role") for row in hub.status().get("agents", []))
                            == sorted(AGENT_ROLES),
                            GUEST_SETTLE_S,
                        ),
                        f"status={hub.status()}",
                    )
                else:
                    case.note("registry assertions SKIPPED: vsockd --status-port is not in this build")
                case.invariant(
                    "no guest rebooted across the hub restart",
                    all("rebooting" not in stream for stream in transcript(True)["streams"].values()),
                    f"streams={ {role: _snippet(stream) for role, stream in transcript(True)['streams'].items()} }",
                )
                case.invariant(
                    "the fleet is still running after the hub restart",
                    (lab.get(run_id) if run_id else {}).get("state") == "running",
                )

                # Teardown: DELETE (through the original noded) kills the fleet
                # and the restarted hub drains.
                reply = lab.delete(run_id)
                case.invariant(
                    "DELETE kills the fleet run",
                    reply.json.get("state") == "killed" and len(reply.json.get("vms", [])) == len(AGENT_ROLES),
                    f"state={reply.json.get('state')}",
                )
                case.invariant("no VM hit the kill timeout", "did not stop" not in lab.noded_log_text())
                leftover = _await_muxer_gone(lab, cids)
                case.invariant("no muxer socket is left behind", not leftover, f"stale={leftover}")
                if status:
                    case.invariant(
                        "the restarted hub's registry drains after the fleet is killed",
                        _await_no_rows(hub),
                        f"status={hub.status()}",
                    )
            finally:
                hub.stop()
            case.invariant("the daemon still serves after the teardown", lab.ping())

    _guarded(case, body)


def _case_work_dir_freshness(suite: Suite, ctx) -> None:
    """A reused work directory is refused — a regression case.

    New failure mode, surfaced by the 2026-10-11 full-matrix run and added to
    the fixed suite here: the runner was pointed at a work directory a previous
    run had used. A lab opens its daemon logs for *append* and `Lab._await_line`
    scans the whole file, so the previous run's `noded: listening on
    http://127.0.0.1:PORT` line satisfied the wait before the new daemon had
    printed anything; the harness then talked to that dead port and every demo
    cell and the red-team lab failed with `Connection refused`, in ~0.5 s, while
    every daemon was in fact healthy. A previous run's per-CID socket files
    satisfy `Lab._await_listeners` the same way.

    Deterministic, no VM: the fixtures pin the refusal (and the positive), so a
    harness that later trusts a reused directory again fails this case.
    """
    case = suite.case(
        "a reused work directory is refused (a regression case)",
        vector=(
            "deterministic fixtures, no daemon: a fresh (or empty, or absent) directory is accepted, while a "
            "directory holding an earlier run's daemon log (whose `listening on` line would satisfy the "
            "readiness wait against a dead port) or an earlier run's per-CID socket file is refused with the "
            "reason"
        ),
    )
    root = ctx.new_dir("harness-work-dir")
    empty = root / "empty"
    empty.mkdir()
    stale_log = root / "stale-log"
    (stale_log / "demo-logs").mkdir(parents=True)
    (stale_log / "demo-logs" / "noded.log").write_text(
        "noded: listening on http://127.0.0.1:46067 (kernel /boot/vmlinuz-linux, initrd ...)\n"
    )
    stale_socket = root / "stale-socket"
    (stale_socket / "demo-uds").mkdir(parents=True)
    (stale_socket / "demo-uds" / "vm10_5000").touch()
    stale_other = root / "stale-other"
    stale_other.mkdir()
    (stale_other / "notes.txt").write_text("an operator's note left in a previous run's directory\n")
    case.invariant(
        "a work directory that does not exist yet is accepted (the runner makes one)",
        work_dir_reuse_reason(root / "absent") is None,
        f"{root / 'absent'}",
    )
    case.invariant(
        "an empty work directory is accepted (an operator may pre-create one)",
        work_dir_reuse_reason(empty) is None,
        f"{empty}",
    )
    log_reason = work_dir_reuse_reason(stale_log)
    case.invariant(
        "a directory holding an earlier run's daemon log is refused, naming the readiness hazard",
        log_reason is not None
        and "demo-logs" in log_reason
        and "readiness wait" in log_reason
        and "dead port" in log_reason,
        f"reason={log_reason!r}",
    )
    socket_reason = work_dir_reuse_reason(stale_socket)
    case.invariant(
        "a directory holding an earlier run's socket file is refused too (existence is not a listener)",
        socket_reason is not None
        and "demo-uds" in socket_reason
        and "not a live listener" in socket_reason,
        f"reason={socket_reason!r}",
    )
    unnamed_reason = work_dir_reuse_reason(stale_other)
    case.invariant(
        "any other leftover still refuses the directory (only a directory the runner can call its own is used)",
        unnamed_reason is not None and "leftovers from an earlier run" in unnamed_reason,
        f"reason={unnamed_reason!r}",
    )
    case.note(
        "the refusal is at the runner's door (Ctx): run.py reports it as BLOCKED (exit 2) with the reason, so a "
        "reused directory can never silently mix two runs' artifacts into one result row."
    )


# --------------------------------------------------------------------- the run


def run(ctx) -> Suite:
    suite = Suite(
        "fleet",
        "every case passes; a case whose required flag/binary is absent is BLOCKED with the reason (never a pass)",
        ctx,
    )
    reason = ctx.can_run("fleet")
    if reason:
        suite.blocked(reason)
        return suite
    status = _flag(ctx, "vsockd", "--status-port")
    hub_ok = _flag(ctx, "noded", "--hub-status")
    suite.note(
        "vsockd --status-port: "
        + ("present (the registry assertions run)" if status else "ABSENT (flagged; the registry assertions are SKIPPED with the reason)")
    )
    suite.note(
        "noded --agents: "
        + (
            "present (the real-guest fleet case runs)"
            if _flag(ctx, "noded", "--agents")
            else "ABSENT (flagged; the real-guest case is SKIPPED with the reason)"
        )
    )
    suite.note(
        "noded --hub-status: "
        + (
            "present (the one-origin proxy case runs)"
            if hub_ok
            else "ABSENT (flagged; the proxy case is SKIPPED with the reason)"
        )
    )
    _case_lifecycle(suite, ctx, status)
    _case_hub_restart(suite, ctx, status)
    _case_readoption(suite, ctx, status)
    _case_reap_and_hangup(suite, ctx, status)
    _case_status_surface(suite, ctx)
    _case_hub_proxy(suite, ctx, hub_ok)
    _case_real_guest(suite, ctx, status)
    _case_work_dir_freshness(suite, ctx)
    return suite
