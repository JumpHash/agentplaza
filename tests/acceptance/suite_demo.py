"""Suite 1 — the hidden demo-task matrix.

The "hidden acceptance test" of the MVP is the frozen session the plaza's own
design calls the demo task: boot N microVMs through the node daemon, have each
guest's agent harness talk the frozen vsock protocol to the orchestrator
stand-in, and see every run reach ``done`` with every VM ``exited``.

The parameters below are the *suite's own*, never passed in — that is what
makes them hidden, and why the same runner proves the same thing on every
machine class. Every deviation fails its cell: a missing session, a wrong CID,
a run that did not finish, a socket left behind.

The five "tasks" one cell is judged on are the five protocol steps of one
session (handshake, echo, uptime, bye, and the guest-side transcript); the
fixed bar is at least four of five per cell, plus the run-level invariants,
which are unconditional.
"""

from __future__ import annotations

import json
import re
import time

from apz import RUN_DEADLINE_S, Lab, Suite, now, reconstruct_console

#: The hidden demo-task matrix parameters.
MATRIX = {
    "n": (1, 2, 4),
    "vcpus": (1, 2),
    "memory_mib": (128, 512),
    "cid_base": (10, 100),
    "beats": (None, 2),
}

#: The orchestrator's handshake echo (the frozen protocol).
WELCOME_ECHO = "plaza-orchestrator"

#: How many churn cycles the reuse case runs over the same CIDs and paths.
CHURN_CYCLES = 5

#: How long a cell's sockets are given to unlink after its run finishes.
SOCKET_SETTLE_S = 5.0


def _matrix_cells() -> list[dict]:
    cells = []
    for n in MATRIX["n"]:
        for vcpus in MATRIX["vcpus"]:
            for memory_mib in MATRIX["memory_mib"]:
                for base in MATRIX["cid_base"]:
                    for beats in MATRIX["beats"]:
                        cells.append(
                            {
                                "n": n,
                                "vcpus": vcpus,
                                "memory_mib": memory_mib,
                                "cid_base": base,
                                "beats": beats,
                            }
                        )
    return cells


def _span(base: int, n: int) -> list[int]:
    return [base + index for index in range(n)]


class Session:
    """One guest session, reconstructed from the orchestrator's log."""

    def __init__(self, cid: int):
        self.cid = cid
        self.hello: dict | None = None
        self.welcome: str | None = None
        self.calls: list[dict] = []
        self.results: list[dict] = []
        self.bye: dict | None = None
        self.clean: int | None = None
        self.vm: int | None = None

    def result_for(self, call: int) -> dict | None:
        for item in self.results:
            if item.get("call") == call:
                return item
        return None

    def call_named(self, name: str) -> dict | None:
        for item in self.calls:
            if item.get("name") == name:
                return item
        return None


def parse_sessions(log: str, cids: list[int]) -> dict[int, Session]:
    """Every session in ``log`` for ``cids``, keyed by guest CID."""
    sessions = {cid: Session(cid) for cid in cids}
    for line in log.splitlines():
        guest = re.match(r"\[guest (\d+) vm(\d+|\?) ->\] (\{.*\})\s*$", line)
        if guest:
            cid, vm, payload = int(guest.group(1)), guest.group(2), guest.group(3)
            session = sessions.get(cid)
            if session is None:
                continue
            if vm != "?":
                session.vm = int(vm)
            _absorb_guest(session, payload)
            continue
        host = re.match(r"\[<-] guest (\d+) vm(\d+) (\{.*\})\s*$", line)
        if host:
            cid, payload = int(host.group(1)), host.group(3)
            session = sessions.get(cid)
            if session is not None:
                _absorb_host(session, payload)
            continue
        ended = re.match(r"vsockd: session with guest (\d+) vm (\d+) ended cleanly after (\d+) tool calls", line)
        if ended:
            session = sessions.get(int(ended.group(1)))
            if session is not None:
                session.clean = int(ended.group(3))
    return sessions


def _absorb_guest(session: Session, payload: str) -> None:
    try:
        message = json.loads(payload)
    except ValueError:
        return
    kind = message.get("type")
    if kind == "hello":
        session.hello = message
    elif kind == "tool_call":
        session.calls.append(message)
    elif kind == "bye":
        session.bye = message


def _absorb_host(session: Session, payload: str) -> None:
    try:
        message = json.loads(payload)
    except ValueError:
        return
    kind = message.get("type")
    if kind == "welcome":
        session.welcome = message.get("echo")
    elif kind == "tool_result":
        session.results.append(message)


def _numeric_pair(text: str) -> bool:
    fields = text.split()
    if len(fields) < 2:
        return False
    try:
        float(fields[0]), float(fields[1])
    except ValueError:
        return False
    return True


def _await_sessions(lab: Lab, cids: list[int], log_from: int, timeout: float = 8.0) -> dict[int, Session]:
    """The run's sessions, once every one of them has ended cleanly.

    The orchestrator logs each session on its own thread, so the last line can
    trail the guest's own reboot by a moment; waiting for it is what makes the
    check deterministic rather than a race on a log flush.
    """
    deadline = now() + timeout
    while True:
        sessions = parse_sessions(lab.vsockd_log_text()[log_from:], cids)
        if all(session.bye and session.clean is not None for session in sessions.values()):
            return sessions
        if now() > deadline:
            return sessions
        time.sleep(0.05)


def judge_cell(lab: Lab, case, spec: dict, log_from: int) -> dict:
    """Drive one matrix cell and record its five tasks and its invariants."""
    cids = _span(spec["cid_base"], spec["n"])
    label = (
        f"n={spec['n']} vcpus={spec['vcpus']} memory_mib={spec['memory_mib']} "
        f"vsock_cid={spec['cid_base']} beats={spec['beats']}"
    )
    case.detail = label
    started = now()
    reply, view = lab.create(
        n=spec["n"],
        vcpus=spec["vcpus"],
        memory_mib=spec["memory_mib"],
        beats=spec["beats"],
        vsock_cid=spec["cid_base"],
    )
    run_id = view["id"]
    case.invariant("POST accepted the run (201)", reply.status == 201, f"status={reply.status}")
    case.invariant(
        "run is running when POST answers",
        view["state"] == "running" and all(vm["state"] == "running" for vm in view["vms"]),
        f"state={view['state']}",
    )
    case.invariant("run has n VMs", len(view["vms"]) == spec["n"], f"vms={len(view['vms'])}")

    final, finished_at = lab.await_state(run_id, {"done", "failed", "killed"}, started + RUN_DEADLINE_S)
    case.invariant(
        "run reached done before the deadline",
        final["state"] == "done",
        f"state={final['state']} after {finished_at - started:.1f}s",
    )
    case.invariant("done run carries ended_at", final["ended_at"] is not None)
    case.invariant(
        "every VM exited",
        len(final["vms"]) == spec["n"] and all(vm["state"] == "exited" for vm in final["vms"]),
        "states=" + ",".join(vm["state"] for vm in final["vms"]),
    )
    case.invariant(
        "per-VM CIDs are base+i",
        [vm["guest_cid"] for vm in sorted(final["vms"], key=lambda vm: vm["id"])] == cids,
        f"cids={[vm['guest_cid'] for vm in final['vms']]}",
    )

    sessions = _await_sessions(lab, cids, log_from)
    for cid in cids:
        session = sessions[cid]
        # Task 1: the handshake, both directions.
        handshake = bool(
            session.hello
            and session.hello.get("guest_cid") == cid
            and session.hello.get("vm") == cid - spec["cid_base"]
            and session.welcome == WELCOME_ECHO
        )
        case.task(
            f"handshake cid {cid}",
            handshake,
            f"hello={session.hello} welcome={session.welcome!r}",
        )
        # Task 2: the echo tool call, verified end to end.
        wanted = f"plaza-guest-{cid}-hello"
        call = session.call_named("echo")
        result = session.result_for(call["call"]) if call else None
        case.task(
            f"echo cid {cid}",
            bool(
                call
                and call.get("args", {}).get("text") == wanted
                and result
                and result.get("ok") is True
                and result.get("output") == wanted
            ),
            f"call={call} result={result}",
        )
        # Task 3: the uptime tool call.
        uptime_call = session.call_named("uptime")
        uptime_result = session.result_for(uptime_call["call"]) if uptime_call else None
        case.task(
            f"uptime cid {cid}",
            bool(uptime_call and uptime_result and uptime_result.get("ok") is True and _numeric_pair(str(uptime_result.get("output", "")))),
            f"output={(uptime_result or {}).get('output')!r}",
        )
        # Task 4: bye, counted, and a clean session end.
        case.task(
            f"bye cid {cid}",
            bool(session.bye and session.bye.get("calls") == 2 and session.clean == 2),
            f"bye={session.bye} clean={session.clean}",
        )
        # Task 5: the guest's own transcript (the other side of the wire). The
        # guest console is shared with the kernel and the init, so a line can
        # be split mid-way; the reconstructed view re-concatenates it.
        serial = next(vm.get("serial", "") for vm in final["vms"] if vm["guest_cid"] == cid)
        guest_stream = reconstruct_console(serial)
        case.task(
            f"guest transcript cid {cid}",
            (
                "agent-harness: handshake complete" in guest_stream
                and f'echo call 1 verified: "{wanted}"' in guest_stream
                and "agent-harness: session complete after 2 tool calls" in guest_stream
                and "plaza guest: agent harness session complete, rebooting" in guest_stream
            ),
            f"serial_bytes={len(serial)}",
        )
        # Hermeticity: each capture is fresh, from this VM's own boot.
        case.invariant(
            f"fresh capture cid {cid}",
            serial.count("Linux version") == 1
            and guest_stream.count("plaza guest: agent harness session complete, rebooting") == 1,
            f"boot banners={serial.count('Linux version')}",
        )

    # The run's sockets are gone once the VMs are; nothing stale is left for
    # the next cell over the same CIDs (the churn case relies on this).
    stale = _await_sockets_gone(lab, cids)
    case.invariant("no leftover vsock sockets", not stale, f"leftover={stale}")
    return {"run": run_id, "state": final["state"], "cids": cids}


def _await_sockets_gone(lab: Lab, cids: list[int], settle: float = SOCKET_SETTLE_S) -> list[str]:
    deadline = now() + settle
    wanted = {f"vm{cid}" for cid in cids}
    while True:
        stale = sorted(wanted & set(lab.uds_entries()))
        if not stale or now() > deadline:
            return stale
        time.sleep(0.05)


def run(ctx) -> Suite:
    suite = Suite(
        "demo-task-matrix",
        "at least 4 of 5 protocol tasks pass per cell, plus every run invariant",
        ctx,
    )
    reason = ctx.can_run("demo")
    if reason:
        suite.blocked(reason)
        return suite
    started = now()
    cells = _matrix_cells()
    served = sorted(set(_span(10, 4)) | set(_span(100, 4)))
    with Lab(ctx, served, "demo") as lab:
        suite.note(f"matrix: {len(cells)} cells over the served CIDs {served}")
        for spec in cells:
            case = suite.case(
                f"cell n={spec['n']} vcpus={spec['vcpus']} mem={spec['memory_mib']} "
                f"cid={spec['cid_base']} beats={spec['beats']}",
                required_tasks=4,
            )
            mark = len(lab.vsockd_log_text())
            try:
                judge_cell(lab, case, spec, mark)
            except Exception as err:  # a cell must fail, not abort the matrix
                case.invariant("cell completed without a harness exception", False, f"{type(err).__name__}: {err}")

        # (b) two runs at once through one daemon and one vsockd, disjoint CIDs.
        case = suite.case("concurrent runs (2 x 4 VMs, disjoint CID ranges)")
        mark = len(lab.vsockd_log_text())
        try:
            _, first = lab.create(n=4, vcpus=1, memory_mib=128, beats=None, vsock_cid=10)
            _, second = lab.create(n=4, vcpus=1, memory_mib=128, beats=None, vsock_cid=100)
            same_time = {first["id"]: first["state"], second["id"]: second["state"]}
            case.invariant(
                "both runs are live at once",
                all(state == "running" for state in same_time.values()),
                f"states={same_time}",
            )
            first_final, _ = lab.await_state(first["id"], {"done", "failed", "killed"}, now() + RUN_DEADLINE_S)
            second_final, _ = lab.await_state(second["id"], {"done", "failed", "killed"}, now() + RUN_DEADLINE_S)
            case.invariant("run A reached done", first_final["state"] == "done", first_final["state"])
            case.invariant("run B reached done", second_final["state"] == "done", second_final["state"])
            sessions = _await_sessions(lab, _span(10, 4) + _span(100, 4), mark)
            case.invariant(
                "all 8 sessions completed on disjoint CIDs",
                all(session.clean == 2 for session in sessions.values()),
                "clean=" + ",".join(f"{cid}:{session.clean}" for cid, session in sessions.items()),
            )
        except Exception as err:
            case.invariant("concurrent case completed without an exception", False, f"{type(err).__name__}: {err}")

        # (c) churn: run -> done -> re-run over the same CIDs and UDS paths.
        case = suite.case(f"churn over reused CIDs/paths ({CHURN_CYCLES} cycles)")
        mark = len(lab.noded_log_text())
        try:
            for cycle in range(CHURN_CYCLES):
                _, view = lab.create(n=2, vcpus=1, memory_mib=128, beats=None, vsock_cid=10)
                final, _ = lab.await_state(view["id"], {"done", "failed", "killed"}, now() + RUN_DEADLINE_S)
                stale = _await_sockets_gone(lab, _span(10, 2))
                case.invariant(
                    f"cycle {cycle} done and clear",
                    final["state"] == "done" and not stale,
                    f"state={final['state']} stale={stale}",
                )
            log = lab.noded_log_text()[mark:]
            case.invariant(
                "no EADDRINUSE anywhere in the churn",
                "Address already in use" not in log and "failed to boot" not in log,
            )
        except Exception as err:
            case.invariant("churn case completed without an exception", False, f"{type(err).__name__}: {err}")

    suite.duration_s = now() - started
    return suite
