"""Suite 7 — the plaza's social layer, driven at its own APIs: the public board
and its read cursors, the contact lists, the conversations between two real
agents, and the social pass the dispatcher runs over them.

`docs/agents/social.md` is the pinned contract; `crates/noded/src/social.rs` is
its implementation. Four cases, each with the bar the suite states:

1. **the board** — a post is recorded *for a connected agent* (the author is the
   agent, the caller is the driver acting for it), a post may link a task the
   ledger holds and never one it does not, the response is bounded
   (`total`/`limit`/`truncated`), and the per-agent read cursors behave exactly
   as the contract says: an advance drops the unread count to zero, a cursor
   behind the agent's own is refused (a cursor never rolls back), a replay is
   idempotent, and one past the newest post is refused.
2. **the contacts** — an explicit add is one-way (the peer's own list is
   untouched), idempotent, and keeps the kind a link was first recorded with; an
   agent is never its own contact; and a **real award** on a two-role fleet
   seeds the link both ways (`hired` for the awarder, `was-hired` for the
   contractor, one `since`).
3. **the conversations** — a say between two connected agents is recorded and
   delivered, and the peer's own `/workspace/messages.log` reads back
   **byte-exact** through the hub's `/tell`; a say the peer's session cannot
   take is recorded with `delivered: false` and the hub's own reason, never
   hidden; the pair's thread and the `ACTIVE_SECS` activity window are read off
   the record.
4. **the social pass** — `dispatchd --once` runs one social pass over the
   plaza's own registers: it picks an idle connected agent, reads the board
   (its cursor advances), posts the interest note for an open task of that
   agent's role (the subject is the contract's `available for <task>`), and the
   claim follows. Gated on the `dispatchd` binary and its flags via `--help`
   probes: a build without them reports the case BLOCKED with the reason.

The split with `suite_agora`: **this** suite owns the plaza's *talking* (the
board, the cursors, the contacts, the conversations and the driver that speaks
for an agent). `suite_agora` owns the *market* (the calls, the bids, the award
and its hiring integration — the board's hiring note and the escrow entry in
the treasury journal). The award is driven here only as the vector for the
contact links it seeds, and its market side is asserted there.

Every lab is the suite's own (`tests/acceptance`'s pattern): fresh daemons on
ephemeral ports, a fresh work directory, CIDs disjoint from every other suite's
(160..178 here; the fleet suite uses 50..56/100..101, the treasury suite
130..151, the agora suite 180..183).
"""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import time
from pathlib import Path

from apz import (
    Lab,
    Suite,
    now,
    reconstruct_console,
    sha256_file,
)

#: The CIDs this suite serves — one block per lab, all of them disjoint from
#: every other suite's CIDs so a stray probe or a leaked guest can never be
#: mistaken for one of ours.
CID_BOARD_A = 170
CID_BOARD_B = 171
CID_CONTACT_ARCHITECT = 172
CID_CONTACT_BUILDER = 173
CID_CONTACT_LIBRARIAN = 174
CID_SAY_BUILDER = 175
CID_SAY_LIBRARIAN = 176
CID_PASS_BUILDER = 177
CID_PASS_LIBRARIAN = 178

#: A CID no lab of this suite serves: the register's "no live agent" refusal.
#: Inside the transport's guest-CID range, so the daemon checks it as a CID and
#: not as a malformed segment.
CID_UNSERVED = 999

#: The tools the codex grants each role this suite runs, transcribed from
#: ``docs/agents/initial-roles.json`` (the treasury suite's transcription, for
#: the same roles). The builder is granted the file tools; the librarian's
#: declared tools are the codex's, of which the harness implements only
#: ``search`` — so under the policy a librarian may search and may not read or
#: write a file (``crates/vsockd/src/roles.rs``).
ARCHITECT_TOOLS = ["bash", "read_file", "list", "search", "transcript_search", "read_artifact", "submit_design"]
BUILDER_TOOLS = ["bash", "read_file", "write_file", "edit_file", "list", "search", "fetch_url"]
LIBRARIAN_TOOLS = ["read_memory", "search", "transcript_search", "read_artifact", "submit_memory_change"]

#: The harness tools the builder's grants cover, as ``dispatchd``'s grant table
#: composes them for an interest note's ``tools:`` clause (the table's order,
#: diagnostics left out and ``conversation`` not in that table at all):
#: ``crates/dispatchd/src/roles.rs`` ``TOOL_GRANTS`` filtered by
#: ``covered_tools`` (``crates/dispatchd/src/social.rs``).
COVERED_BUILDER_TOOLS = "read_file, write_file, edit_file, append_file, list, search"

#: The bounds the contract states, transcribed from ``crates/noded/src/social.rs``
#: (with the crate's own names) so this suite asserts the numbers that are
#: actually enforced rather than numbers of its own.
BOARD_LIMIT = 256  # BOARD_LIMIT
MAX_POSTS = 1024  # MAX_POSTS (the board's own store cap; a fresh post past it is refused)
CONTACTS_LIMIT = 256  # MAX_CONTACTS_PER_AGENT
MAX_SUBJECT = 256  # MAX_SUBJECT
MAX_BODY = 4096  # MAX_BODY
MAX_SAY_TEXT = 2048  # MAX_SAY_TEXT
CONVERSATIONS_LIMIT = 256  # CONVERSATIONS_LIMIT
ACTIVE_SECS = 120  # ACTIVE_SECS

#: The `posted_by` a post gets when its caller names no driver (the contract's
#: own word for the caller that acts for an agent): ``DEFAULT_POSTED_BY``.
DEFAULT_POSTED_BY = "driver"

#: The `posted_by` every post the social pass makes carries (``DRIVER``).
DISPATCHD_DRIVER = "dispatchd"

#: How long a real role guest is given to boot, greet the hub, mount its volume
#: and beat.
GUEST_SETTLE_S = 60.0

#: How long a log transition or a registry change is given.
SETTLE_S = 15.0

#: How long one `dispatchd --once` pass may take: a handful of bounded loopback
#: calls plus the drive they plan.
DISPATCHD_TIMEOUT_S = 90.0

#: How long the workspace image builder may take (the budgets suite's bound).
IMAGE_BUILD_TIMEOUT_S = 120.0

#: A guest log line: `<iso utc> from <role> (cid <N>): <text>`
#: (``crates/agent-harness/src/files.rs`` ``message_line``).
MESSAGE_LINE = re.compile(
    r"(?P<stamp>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z) from (?P<role>.*) \(cid (?P<cid>\d+)\): (?P<text>.*)"
)


# ------------------------------------------------------------- small helpers


def _flag(ctx, binary: str, needle: str) -> bool:
    """Whether ``binary``'s CLI advertises ``needle`` (a ``--help`` probe).

    A binary the build does not produce at all is ``False`` rather than a
    harness error: the social pass is gated on ``dispatchd`` being built and
    advertising its flags, and a case whose gate is absent is reported BLOCKED
    with the reason (the fleet/treasury ``--help`` probe idiom, made total).
    """
    path = Path(ctx.bin_dir) / binary
    if not path.exists():
        return False
    try:
        out = subprocess.run([str(path), "--help"], capture_output=True, text=True, timeout=20, check=False)
    except (OSError, subprocess.SubprocessError):
        return False
    return needle in out.stdout


def _guarded(case, body) -> None:
    """Run a case body; a harness exception fails the case, never the suite."""
    try:
        body()
    except Exception as err:  # a case must fail, not abort the run
        case.invariant(
            "the case completed without a harness exception", False, f"{type(err).__name__}: {err}"
        )


def _sleeper(predicate, timeout: float) -> bool:
    """Poll ``predicate`` until it is true or ``timeout`` elapses."""
    deadline = now() + timeout
    while now() < deadline:
        if predicate():
            return True
        time.sleep(0.05)
    return predicate()


def _log_lines(lab: Lab, needle: str) -> list[str]:
    return [line for line in lab.vsockd_log_text().splitlines() if needle in line]


def _snippet(text: str, limit: int = 200) -> str:
    return text[-limit:]


def _row_for(lab: Lab, cid: int) -> dict | None:
    """The hub's ``/status`` registry row for ``cid``, or ``None``."""
    for row in lab.status().get("agents", []):
        if row.get("cid") == cid:
            return row
    return None


def _await_no_row(lab: Lab, cid: int, timeout: float = SETTLE_S) -> bool:
    """``cid`` has drained from the hub's registry."""
    return _sleeper(lambda: _row_for(lab, cid) is None, timeout)


def _role_cids(lab: Lab, run_id: str) -> dict[str, int]:
    """The role each of a fleet run's VMs runs as, mapped to its guest CID."""
    view = lab.get(run_id) if run_id else {}
    return {
        vm["role"]: vm["guest_cid"]
        for vm in view.get("vms", [])
        if vm.get("role") and vm.get("guest_cid")
    }


def _stream_of(lab: Lab, run_id: str, role: str) -> str:
    """A fleet guest's reconstructed console stream (the userspace transcript)."""
    view = lab.get(run_id) if run_id else {}
    for vm in view.get("vms", []):
        if vm.get("role") == role:
            return reconstruct_console(vm.get("serial", ""))
    return ""


def _fleet_run(lab: Lab) -> tuple[str | None, bool]:
    """The initial role fleet's run id from the daemon's own startup line."""
    line = next(
        (candidate for candidate in lab.noded_log_text().splitlines() if "noded: initial role fleet" in candidate),
        "",
    )
    if not line:
        return None, False
    # `noded: initial role fleet <id>: N agents running (roles ..., CIDs a..=b)`
    head = line.split("noded: initial role fleet ", 1)[1]
    return head.split(":", 1)[0], True


def _unread(board: dict, cid: int) -> int | None:
    """An agent's unread count in a board response, or ``None`` if it has none."""
    for entry in board.get("unread", []):
        if entry.get("agent_cid") == cid:
            return entry.get("count")
    return None


def _workspace_image(ctx) -> tuple[Path | None, str | None]:
    """The base workspace image the conversations and the drive need.

    Returns ``(image, None)`` or ``(None, reason)``. The ``conversation`` tool
    appends to the agent's own ``/workspace/messages.log`` and the dispatcher's
    drive writes into the guest's ``/workspace``, and a guest mounts that volume
    only when the VMM gave it a block device (``scripts/mk-initrd.sh``'s
    ``mount_workspace``) — so a fleet booted without ``noded --workspace`` has
    no volume and both answer ``unavailable``. The builder is the repository's
    own ``scripts/mk-workspace-image.sh`` (the budgets suite's), run with the cwd
    at the repository root; when ``mkfs.ext4`` is missing the reason says so and
    the case is BLOCKED rather than absent.
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


def _save(ctx, subdir: str, name: str, text: str) -> Path:
    """Persist one piece of evidence and register it with the runner.

    The runner copies a bounded excerpt of every registered artifact into
    ``results/evidence/<stamp>/``, so a case's assertions are reproducible from
    the committed evidence rather than only from its own verdict.
    """
    path = ctx.new_dir(f"social-evidence-{subdir}") / name
    path.write_text(text)
    ctx.register_artifact(path)
    return path


def _write_manifest(ctx, tag: str, roles: dict[str, list[str]]) -> Path:
    """A roles manifest in this suite's work dir, readable by both daemons.

    ``noded --agents`` boots one keepalive VM per role; ``vsockd --roles``
    enforces the codex's tool grants; ``dispatchd --roles`` reads the same
    grants to plan. One document all three can read is what makes a single lab a
    fleet *and* a policy (the treasury suite's idiom).
    """
    entries = {
        name: {
            "role": name,
            "title": f"The {name}",
            "permits": [],
            "toolsets": [],
            "budget": 1,
            "variant": None,
            "tools": tools,
        }
        for name, tools in roles.items()
    }
    path = ctx.new_dir(f"social-manifest-{tag}") / "roles.json"
    path.write_text(json.dumps({"source": "tests/acceptance/suite_social.py", "roles": entries}, indent=2) + "\n")
    ctx.register_artifact(path)
    return path


def _connected(lab: Lab, pairs: tuple[tuple[str, int], ...]) -> bool:
    """Every named role's guest has greeted the hub, as the hub's log says."""
    text = lab.vsockd_log_text()
    return all(f"vsockd: agent {cid} role {role} connected" in text for role, cid in pairs)


# ------------------------------------------------------------------- the cases


def _case_board(suite: Suite, ctx, fleet_ok: bool) -> None:
    case = suite.case(
        "the board over the real API: the connected author, the task link, the bounded read, and the read cursors",
        vector=(
            "noded --agents <builder + librarian>: the empty board; a post for an unconnected CID is the "
            "register's own 409, verbatim; a post naming a task the ledger does not hold is a 400; the "
            "subject/body caps; a connected agent's post is p1 with the author's own role and the driver's "
            "name; POST /board/read drops the unread count to zero and the board's own unread line agrees; a "
            "cursor behind the agent's own is refused 409 naming the cursor (a cursor never rolls back); a "
            "replay is idempotent; a cursor past the newest post is refused 409; and the response is bounded "
            "(257 posts -> the 256 newest, truncated said so)"
        ),
    )
    if not fleet_ok:
        case.blocked(
            "noded --agents is not in this build's CLI (probe: noded --help); the role fleet has not landed, "
            "so no real agent can author a post and the board case cannot run here"
        )
        return

    def body() -> None:
        manifest = _write_manifest(ctx, "board", {"builder": BUILDER_TOOLS, "librarian": LIBRARIAN_TOOLS})
        with Lab(
            ctx,
            [CID_BOARD_A, CID_BOARD_B],
            "social-board",
            agents=manifest,
            agents_cid_base=CID_BOARD_A,
            hub_status="off",
            hub_admin="off",
        ) as lab:
            run_id, booted = _fleet_run(lab)
            case.invariant("noded booted the initial role fleet at startup", booted, lab.noded_log_text()[-400:])
            roles = _role_cids(lab, run_id) if run_id else {}
            builder = roles.get("builder")
            librarian = roles.get("librarian")
            case.invariant(
                "the two-role fleet boots one VM per role, in alphabetical order, at the CIDs the base gives them",
                builder == CID_BOARD_A and librarian == CID_BOARD_B,
                f"roles={roles}",
            )
            connected = _sleeper(
                lambda: _connected(lab, (("builder", CID_BOARD_A), ("librarian", CID_BOARD_B))), GUEST_SETTLE_S
            )
            case.invariant("both role guests connected to the hub", connected, f"connected={_log_lines(lab, 'connected')}")
            if builder is None or librarian is None:
                return

            def board_view() -> dict:
                reply = lab.api_get("/api/v0/board")
                if reply.status != 200:
                    raise AssertionError(f"GET /api/v0/board: {reply.status} {reply.body[:200]}")
                return reply.json

            # --- the board is empty, bounded, and both agents are at zero ------
            board = board_view()
            case.invariant(
                "the board starts empty: no posts, total 0, the endpoint's limit, untruncated, and both connected agents at 0 unread",
                board["posts"] == []
                and board["total"] == 0
                and board["limit"] == BOARD_LIMIT
                and board["truncated"] is False
                and sorted(entry["agent_cid"] for entry in board["unread"]) == [CID_BOARD_A, CID_BOARD_B]
                and all(entry["count"] == 0 for entry in board["unread"]),
                f"board={board}",
            )

            # --- the author must be a connected agent -------------------------
            stray = lab.api_post(
                "/api/v0/board", {"agent_cid": CID_UNSERVED, "subject": "anyone there?", "body": "hello"}
            )
            case.invariant(
                "a post for an agent this node has no live session for is the register's own 409, word for word",
                stray.status == 409 and stray.json.get("error") == f"no live agent has guest CID {CID_UNSERVED}",
                f"{stray.status} {stray.body[:200]}",
            )

            # --- the task link is checked against the ledger -------------------
            dangling = lab.api_post(
                "/api/v0/board",
                {"agent_cid": builder, "subject": "the work", "body": "see the call", "task": "t9"},
            )
            case.invariant(
                "a post naming a task the ledger does not hold is a 400 in the board's own words (the board must not lie about work)",
                dangling.status == 400 and dangling.json.get("error") == "no task with id t9",
                f"{dangling.status} {dangling.body[:200]}",
            )

            # --- the caps, and the method contract ----------------------------
            outside = {
                "an empty subject": ({"agent_cid": builder, "subject": "   ", "body": "b"}, "subject must not be empty"),
                "an empty body": ({"agent_cid": builder, "subject": "s", "body": ""}, "body must not be empty"),
                "a subject over the cap": (
                    {"agent_cid": builder, "subject": "x" * (MAX_SUBJECT + 1), "body": "b"},
                    f"subject must be at most {MAX_SUBJECT} bytes",
                ),
                "a body over the cap": (
                    {"agent_cid": builder, "subject": "s", "body": "x" * (MAX_BODY + 1)},
                    f"body must be at most {MAX_BODY} bytes",
                ),
            }
            observed = {}
            for label, (payload, expected) in outside.items():
                reply = lab.api_post("/api/v0/board", payload)
                observed[label] = (reply.status, reply.json.get("error"), expected)
            case.invariant(
                "a post outside the board's caps is a 400 naming the field and the cap it broke",
                all(
                    status == 400 and error == expected
                    for status, error, expected in observed.values()
                ),
                f"{observed}",
            )
            case.invariant(
                "the board takes GET and POST for a post, and POST only for a read",
                lab.api_request("PUT", "/api/v0/board", {}).status == 405
                and lab.api_request("DELETE", "/api/v0/board").status == 405
                and lab.api_get("/api/v0/board/read").status == 405,
                f"put={lab.api_request('PUT', '/api/v0/board', {}).status} get_read={lab.api_get('/api/v0/board/read').status}",
            )

            # --- a connected agent posts: p1 ----------------------------------
            first = lab.api_post(
                "/api/v0/board", {"agent_cid": builder, "subject": "the plaza is open", "body": "come and work"}
            )
            case.invariant(
                "a post for a connected agent is a 201: p1, the author's own role, no task, the contract's driver default, and a timestamp",
                first.status == 201
                and first.json.get("id") == "p1"
                and first.json.get("agent_cid") == builder
                and first.json.get("role") == "builder"
                and first.json.get("subject") == "the plaza is open"
                and first.json.get("body") == "come and work"
                and first.json.get("task") is None
                and first.json.get("posted_by") == DEFAULT_POSTED_BY
                and first.json.get("created_at", 0) > 1_700_000_000_000,
                f"{first.status} {first.body[:240]}",
            )
            created = lab.api_post("/api/v0/tasks", {"goal": "the posted work", "role": "builder"})
            task_id = created.json.get("id")
            second = lab.api_post(
                "/api/v0/board",
                {
                    "agent_cid": librarian,
                    "subject": "who wants work",
                    "body": "the call is up",
                    "task": task_id,
                    "posted_by": "the-suite",
                },
            )
            case.invariant(
                "a post naming a task the ledger holds is recorded with the link and the driver the caller named",
                second.status == 201
                and second.json.get("id") == "p2"
                and second.json.get("agent_cid") == librarian
                and second.json.get("role") == "librarian"
                and second.json.get("task") == task_id
                and second.json.get("posted_by") == "the-suite",
                f"{second.status} {second.body[:240]}",
            )
            board = board_view()
            case.invariant(
                "the board reads newest first, with total == the posts it holds and no truncation",
                [post["id"] for post in board["posts"]] == ["p2", "p1"]
                and board["total"] == 2
                and board["limit"] == BOARD_LIMIT
                and board["truncated"] is False,
                f"posts={[post['id'] for post in board['posts']]} total={board['total']}",
            )
            case.invariant(
                "both connected agents are two posts behind (unread counts are derived, never stored)",
                _unread(board, builder) == 2 and _unread(board, librarian) == 2,
                f"unread={board['unread']}",
            )

            # --- the read cursor ----------------------------------------------
            read = lab.api_post("/api/v0/board/read", {"agent_cid": builder, "up_to": "p2"})
            case.invariant(
                "the read advances the agent's cursor and drops its unread count to zero",
                read.status == 200
                and read.json.get("agent_cid") == builder
                and read.json.get("up_to") == "p2"
                and read.json.get("unread") == 0,
                f"{read.status} {read.body[:200]}",
            )
            board = board_view()
            case.invariant(
                "the board's own unread line agrees: the reader is at zero and the other agent is not",
                _unread(board, builder) == 0 and _unread(board, librarian) == 2,
                f"unread={board['unread']}",
            )
            backwards = lab.api_post("/api/v0/board/read", {"agent_cid": builder, "up_to": "p1"})
            case.invariant(
                "a cursor behind the agent's own is refused 409 naming the cursor it holds: a cursor never rolls back",
                backwards.status == 409
                and backwards.json.get("error") == "up_to p1 is behind the agent's read cursor p2",
                f"{backwards.status} {backwards.body[:200]}",
            )
            replay = lab.api_post("/api/v0/board/read", {"agent_cid": builder, "up_to": "p2"})
            case.invariant(
                "replaying the cursor the agent already holds is idempotent: the same answer, and the cursor unmoved",
                replay.status == 200
                and replay.json == read.json
                and _unread(board_view(), builder) == 0,
                f"replay={replay.body[:160]} first={read.body[:160]}",
            )
            beyond = lab.api_post("/api/v0/board/read", {"agent_cid": librarian, "up_to": "p9"})
            case.invariant(
                "a cursor past the newest post is refused 409, naming the newest post",
                beyond.status == 409 and beyond.json.get("error") == "up_to p9 is past the newest post p2",
                f"{beyond.status} {beyond.body[:200]}",
            )
            malformed = lab.api_post("/api/v0/board/read", {"agent_cid": librarian, "up_to": "nope"})
            zero = lab.api_post("/api/v0/board/read", {"agent_cid": librarian, "up_to": "p0"})
            case.invariant(
                "a cursor that names no post is a 400 (and p0 is not a post: the first is p1)",
                malformed.status == 400
                and malformed.json.get("error") == "up_to must name a post (p<seq>), not nope"
                and zero.status == 400
                and zero.json.get("error") == "up_to must name a post: the first post is p1",
                f"malformed={malformed.status} {malformed.body[:120]} zero={zero.status} {zero.body[:120]}",
            )
            case.invariant(
                "a refused read changes nothing: the reader's cursor is where it was",
                _unread(board_view(), builder) == 0 and _unread(board_view(), librarian) == 2,
                f"unread={board_view()['unread']}",
            )

            # --- the response is bounded --------------------------------------
            for index in range(BOARD_LIMIT + 1 - board["total"]):
                lab.api_post(
                    "/api/v0/board",
                    {"agent_cid": builder, "subject": f"the full board {index}", "body": "x"},
                )
            board = board_view()
            case.invariant(
                "the board response is bounded: 257 posts, the 256 newest returned (oldest first dropped), truncated said so",
                board["total"] == 257
                and len(board["posts"]) == BOARD_LIMIT
                and board["truncated"] is True
                and board["posts"][0]["id"] == "p257"
                and board["posts"][-1]["id"] == "p2",
                f"total={board['total']} len={len(board['posts'])} first={board['posts'][0]['id']} last={board['posts'][-1]['id']}",
            )

            # --- the board's own store cap is absolute -------------------------
            for _ in range(MAX_POSTS - board["total"]):
                lab.api_post("/api/v0/board", {"agent_cid": builder, "subject": "filling the board", "body": "x"})
            full = lab.api_post("/api/v0/board", {"agent_cid": builder, "subject": "one too many", "body": "x"})
            case.invariant(
                "the board's store cap is absolute: at 1024 posts a fresh post is refused 409 in the board's own words (a post is never dropped)",
                board_view()["total"] == MAX_POSTS
                and full.status == 409
                and full.json.get("error") == f"the board is full ({MAX_POSTS} posts)",
                f"total={board_view()['total']} {full.status} {full.body[:200]}",
            )

            _save(ctx, "board", "board-257.json", json.dumps(board, indent=2) + "\n")
            _save(
                ctx,
                "board",
                "refusals.txt",
                "\n".join(
                    [
                        f"post for an unconnected CID: {stray.status} {stray.body}",
                        f"post naming an unknown task:  {dangling.status} {dangling.body}",
                        f"cursor behind the agent's own: {backwards.status} {backwards.body}",
                        f"cursor past the newest post:   {beyond.status} {beyond.body}",
                        f"cursor naming no post:         {malformed.status} {malformed.body}",
                        f"cursor naming p0:              {zero.status} {zero.body}",
                        f"caps: {observed}",
                    ]
                )
                + "\n",
            )

    _guarded(case, body)


def _case_contacts(suite: Suite, ctx, fleet_ok: bool) -> None:
    case = suite.case(
        "the contact lists: an explicit add is one-way and idempotent, and a real award seeds the link both ways",
        vector=(
            "noded --agents <architect + builder + librarian>, no hub: a connected agent that has met nobody "
            "reads back an empty, bounded list; a CID the daemon has never heard of is a 404; an explicit add "
            "records `met` with the peer's role and leaves the peer's own list untouched; the replay answers "
            "the link that already stands; an agent is never its own contact (409); then a real award on the "
            "builder's call (the architect hires) seeds `hired` for the awarder and `was-hired` for the "
            "contractor with one `since`, and a later handshake keeps the kind the award recorded"
        ),
    )
    if not fleet_ok:
        case.blocked(
            "noded --agents is not in this build's CLI (probe: noded --help); the role fleet has not landed, "
            "so no real agent can be linked and the contacts case cannot run here"
        )
        return

    def body() -> None:
        manifest = _write_manifest(
            ctx,
            "contacts",
            {"architect": ARCHITECT_TOOLS, "builder": BUILDER_TOOLS, "librarian": LIBRARIAN_TOOLS},
        )
        with Lab(
            ctx,
            [CID_CONTACT_ARCHITECT, CID_CONTACT_BUILDER, CID_CONTACT_LIBRARIAN],
            "social-contacts",
            agents=manifest,
            agents_cid_base=CID_CONTACT_ARCHITECT,
            hub_status="off",
            hub_admin="off",
        ) as lab:
            run_id, booted = _fleet_run(lab)
            case.invariant("noded booted the initial role fleet at startup", booted, lab.noded_log_text()[-400:])
            roles = _role_cids(lab, run_id) if run_id else {}
            architect = roles.get("architect")
            builder = roles.get("builder")
            librarian = roles.get("librarian")
            case.invariant(
                "the three-role fleet boots one VM per role, in alphabetical order, at the CIDs the base gives them",
                architect == CID_CONTACT_ARCHITECT
                and builder == CID_CONTACT_BUILDER
                and librarian == CID_CONTACT_LIBRARIAN,
                f"roles={roles}",
            )
            connected = _sleeper(
                lambda: _connected(
                    lab,
                    (
                        ("architect", CID_CONTACT_ARCHITECT),
                        ("builder", CID_CONTACT_BUILDER),
                        ("librarian", CID_CONTACT_LIBRARIAN),
                    ),
                ),
                GUEST_SETTLE_S,
            )
            case.invariant("all three role guests connected to the hub", connected, f"connected={_log_lines(lab, 'connected')}")
            if architect is None or builder is None or librarian is None:
                return

            def contacts(cid: int) -> dict:
                reply = lab.api_get(f"/api/v0/agents/{cid}/contacts")
                if reply.status != 200:
                    raise AssertionError(f"GET /api/v0/agents/{cid}/contacts: {reply.status} {reply.body[:200]}")
                return reply.json

            def add(cid: int, peer: int):
                return lab.api_post(f"/api/v0/agents/{cid}/contacts", {"peer_cid": peer})

            # --- the list this daemon holds ------------------------------------
            held = contacts(builder)
            case.invariant(
                "a connected agent that has met nobody reads back an empty list, bounded and untruncated",
                held["agent_cid"] == builder
                and held["contacts"] == []
                and held["total"] == 0
                and held["limit"] == CONTACTS_LIMIT
                and held["truncated"] is False,
                f"{held}",
            )
            unknown = lab.api_get(f"/api/v0/agents/{CID_UNSERVED}/contacts")
            case.invariant(
                "a CID the store has never heard of and the register does not show is a 404",
                unknown.status == 404
                and unknown.json.get("error") == f"no agent with guest CID {CID_UNSERVED}",
                f"{unknown.status} {unknown.body[:200]}",
            )

            # --- the explicit add: one-way, idempotent -------------------------
            added = add(builder, librarian)
            case.invariant(
                "an explicit add records `met`, with the peer's role and the moment the daemon recorded it",
                added.status == 200
                and added.json.get("peer_cid") == librarian
                and added.json.get("role") == "librarian"
                and added.json.get("kind") == "met"
                and added.json.get("since", 0) > 1_700_000_000_000,
                f"{added.status} {added.body[:200]}",
            )
            case.invariant(
                "the add is one-way: the peer's own list does not hold the agent that added it",
                contacts(librarian)["contacts"] == [],
                f"librarian={contacts(librarian)}",
            )
            again = add(builder, librarian)
            case.invariant(
                "the add is idempotent: the replay answers the link that already stands, unmoved",
                again.status == 200 and again.json == added.json,
                f"again={again.body[:200]} first={added.body[:200]}",
            )
            selfref = add(builder, builder)
            case.invariant(
                "an agent is never its own contact: the daemon's own 409, word for word",
                selfref.status == 409
                and selfref.json.get("error") == f"guest CID {builder} cannot be its own contact",
                f"{selfref.status} {selfref.body[:200]}",
            )
            stray = add(builder, CID_UNSERVED)
            case.invariant(
                "adding a peer this node has no live session for is the register's own 409",
                stray.status == 409 and stray.json.get("error") == f"no live agent has guest CID {CID_UNSERVED}",
                f"{stray.status} {stray.body[:200]}",
            )
            case.invariant(
                "the contact endpoint takes GET and POST only",
                lab.api_request("DELETE", f"/api/v0/agents/{builder}/contacts", {}).status == 405,
                f"delete={lab.api_request('DELETE', f'/api/v0/agents/{builder}/contacts', {}).status}",
            )

            # --- a real award seeds the link both ways -------------------------
            created = lab.api_post(
                "/api/v0/tasks",
                {"goal": "the hired work", "acceptance": "the note is filed", "role": "builder", "budget": 500},
            )
            task_id = created.json.get("id")
            bid = lab.api_post(f"/api/v0/tasks/{task_id}/bid", {"agent_cid": builder, "price": 300})
            case.invariant(
                "the builder bids on its own role's call (the award's precondition)",
                bid.status == 200 and bid.json.get("price") == 300 and bid.json.get("bidder") == "builder",
                f"{bid.status} {bid.body[:200]}",
            )
            awarded = lab.api_post(
                f"/api/v0/tasks/{task_id}/award", {"agent_cid": builder, "awarder_cid": architect}
            )
            case.invariant(
                "the award binds the task through the ledger's own claim",
                awarded.status == 200
                and awarded.json.get("state") == "claimed"
                and awarded.json.get("assignee_id") == "builder"
                and awarded.json.get("agent_cid") == builder,
                f"{awarded.status} {awarded.body[:200]}",
            )
            architect_list = contacts(architect)["contacts"]
            builder_list = contacts(builder)["contacts"]
            hired = next((row for row in architect_list if row["peer_cid"] == builder), None)
            was_hired = next((row for row in builder_list if row["peer_cid"] == architect), None)
            case.invariant(
                "an award seeds the link both ways: `hired` for the awarder, `was-hired` for the contractor",
                hired is not None
                and hired["kind"] == "hired"
                and hired["role"] == "builder"
                and was_hired is not None
                and was_hired["kind"] == "was-hired"
                and was_hired["role"] == "architect",
                f"architect={architect_list} builder={builder_list}",
            )
            case.invariant(
                "the two links carry the same `since`: one hire, one moment",
                hired is not None and was_hired is not None and hired["since"] == was_hired["since"],
                f"hired={hired} was_hired={was_hired}",
            )
            restated = add(architect, builder)
            restated_back = add(builder, architect)
            case.invariant(
                "a later handshake keeps the kind an award recorded: the first kind stands, both ways",
                restated.status == 200
                and restated.json.get("kind") == "hired"
                and restated.json.get("since") == hired["since"]
                and restated_back.status == 200
                and restated_back.json.get("kind") == "was-hired"
                and restated_back.json.get("since") == was_hired["since"],
                f"architect->builder={restated.body[:160]} builder->architect={restated_back.body[:160]}",
            )
            case.invariant(
                "the handshake the suite made earlier stands beside the award's link, in the order they were recorded",
                [row["peer_cid"] for row in contacts(builder)["contacts"]] == [librarian, architect],
                f"builder={contacts(builder)['contacts']}",
            )

            _save(ctx, "contacts", "architect-contacts.json", json.dumps(contacts(architect), indent=2) + "\n")
            _save(ctx, "contacts", "builder-contacts.json", json.dumps(contacts(builder), indent=2) + "\n")
            _save(
                ctx,
                "contacts",
                "refusals.txt",
                "\n".join(
                    [
                        f"unknown CID list:        {unknown.status} {unknown.body}",
                        f"self-contact:            {selfref.status} {selfref.body}",
                        f"unconnected peer:        {stray.status} {stray.body}",
                        f"award:                   {awarded.status} {awarded.body}",
                    ]
                )
                + "\n",
            )

    _guarded(case, body)


def _case_conversations(
    suite: Suite,
    ctx,
    fleet_ok: bool,
    admin_ok: bool,
    roles_ok: bool,
    hubadmin_ok: bool,
    image: Path | None,
    image_reason: str | None,
) -> None:
    case = suite.case(
        "the conversations: a delivered say read back byte-exact from the peer's own log, an undeliverable say recorded with the hub's reason, and the pair's thread",
        vector=(
            "a 2-role fleet (builder + librarian) on the suite's own vsockd --admin-port --roles and noded "
            "--hub-admin --workspace: a say is delivered through the hub and the peer's own "
            "/workspace/messages.log reads back byte-exact through /tell (the builder's, with read_file); the "
            "other direction is delivered too — the channel is the plaza's basis, not a role's privilege, and "
            "the hub's policy is proven live by a 403 on a tool the librarian does not grant; a stopped "
            "session makes the peer undeliverable and the row records delivered:false with the hub's own "
            "words; the self-say and text caps; and the pair thread with the ACTIVE_SECS window"
        ),
    )
    missing = [
        label
        for label, present in (
            ("noded --agents", fleet_ok),
            ("noded --hub-admin", hubadmin_ok),
            ("vsockd --admin-port", admin_ok),
            ("vsockd --roles", roles_ok),
        )
        if not present
    ]
    if missing:
        case.blocked(
            "the conversations need a flag this build's CLI does not advertise (probe: <binary> --help): "
            + ", ".join(missing)
            + " — so the conversations case is SKIPPED"
        )
        return
    if image is None:
        case.blocked(
            "the conversation channel writes the peer's own /workspace/messages.log (the harness's "
            f"`conversation` tool appends to the guest's writable volume) and there is no workspace image: "
            f"{image_reason} — the conversations case is SKIPPED"
        )
        return

    def body() -> None:
        manifest = _write_manifest(ctx, "say", {"builder": BUILDER_TOOLS, "librarian": LIBRARIAN_TOOLS})
        with Lab(
            ctx,
            [CID_SAY_BUILDER, CID_SAY_LIBRARIAN],
            "social-say",
            agents=manifest,
            agents_cid_base=CID_SAY_BUILDER,
            roles=manifest,
            workspace=image,
            status_port=0,
            admin_port=0,
            hub_status="off",
            hub_admin="self",
        ) as lab:
            run_id, booted = _fleet_run(lab)
            case.invariant("noded booted the initial role fleet at startup", booted, lab.noded_log_text()[-400:])
            roles = _role_cids(lab, run_id) if run_id else {}
            builder = roles.get("builder")
            librarian = roles.get("librarian")
            case.invariant(
                "the two-role fleet boots one VM per role, in alphabetical order, at the CIDs the base gives them",
                builder == CID_SAY_BUILDER and librarian == CID_SAY_LIBRARIAN,
                f"roles={roles}",
            )
            connected = _sleeper(
                lambda: _connected(lab, (("builder", CID_SAY_BUILDER), ("librarian", CID_SAY_LIBRARIAN))),
                GUEST_SETTLE_S,
            )
            case.invariant("both role guests connected to the hub", connected, f"connected={_log_lines(lab, 'connected')}")
            if builder is None or librarian is None:
                return
            case.invariant(
                "each guest mounted its writable /workspace volume (noded --workspace; the conversation log lives there)",
                _sleeper(
                    lambda: all("/workspace mounted" in _stream_of(lab, run_id, role) for role in ("builder", "librarian")),
                    GUEST_SETTLE_S,
                ),
                f"builder={_snippet(_stream_of(lab, run_id, 'builder'))}",
            )

            # --- a delivered say: the peer's own log --------------------------
            first_text = f"the plaza says hello {ctx.seed}"
            first = lab.api_post(f"/api/v0/agents/{librarian}/say", {"to_cid": builder, "text": first_text})
            case.invariant(
                "a say between two connected agents is a 201: c1, both roles, the text, delivered true and no reason",
                first.status == 201
                and first.json.get("id") == "c1"
                and first.json.get("from_cid") == librarian
                and first.json.get("to_cid") == builder
                and first.json.get("from_role") == "librarian"
                and first.json.get("to_role") == "builder"
                and first.json.get("text") == first_text
                and first.json.get("delivered") is True
                and first.json.get("delivery") is None
                and first.json.get("created_at", 0) > 1_700_000_000_000,
                f"{first.status} {first.body[:240]}",
            )
            back = lab.admin_tell(builder, "read_file", {"path": "messages.log"})
            parsed = {}
            if back.status == 200 and back.json.get("ok") is True:
                try:
                    parsed = json.loads(back.json.get("output", ""))
                except ValueError:
                    parsed = {}
            content = parsed.get("content", "") if isinstance(parsed, dict) else ""
            line = content[:-1] if content.endswith("\n") else content
            read_line = MESSAGE_LINE.fullmatch(line)
            case.invariant(
                "the peer's own messages.log reads back through the hub's /tell, byte-exact: one line, the speaker's role and CID, the text",
                back.status == 200
                and back.json.get("ok") is True
                and isinstance(parsed, dict)
                and parsed.get("path") == "messages.log"
                and parsed.get("truncated") is False
                and content.endswith("\n")
                and read_line is not None
                and read_line.group("role") == "librarian"
                and int(read_line.group("cid")) == librarian
                and read_line.group("text") == first_text
                and parsed.get("bytes") == len(content.encode())
                and parsed.get("size") == len(content.encode()),
                f"read={back.body[:240]} content={content!r}",
            )

            # --- the other direction: the channel is the plaza's basis --------
            reply_text = f"on my way {ctx.seed}"
            second = lab.api_post(f"/api/v0/agents/{builder}/say", {"to_cid": librarian, "text": reply_text})
            case.invariant(
                "the other direction is delivered too — the channel is the plaza's basis, not a role's privilege (the librarian's grants include neither `conversation` nor `read_file`)",
                second.status == 201
                and second.json.get("id") == "c2"
                and second.json.get("from_cid") == builder
                and second.json.get("to_cid") == librarian
                and second.json.get("delivered") is True
                and second.json.get("delivery") is None,
                f"{second.status} {second.body[:240]}",
            )
            denied = lab.admin_tell(librarian, "read_file", {"path": "messages.log"})
            case.invariant(
                "the hub's role policy is live (the delivery above is the basis grant, not a disabled policy): read_file on the librarian is the hub's own 403",
                denied.status == 403
                and denied.json.get("ok") is False
                and denied.json.get("output") == "role librarian does not grant read_file",
                f"{denied.status} {denied.body[:200]}",
            )
            found = lab.admin_tell(librarian, "search", {"pattern": reply_text})
            searched = {}
            if found.status == 200 and found.json.get("ok") is True:
                try:
                    searched = json.loads(found.json.get("output", ""))
                except ValueError:
                    searched = {}
            matches = searched.get("matches", []) if isinstance(searched, dict) else []
            searched_line = MESSAGE_LINE.fullmatch(matches[0]["text"]) if matches and matches[0].get("text") else None
            case.invariant(
                "the peer that may not read the file still finds its own message line byte-exact (the librarian's `search` on its own log)",
                found.status == 200
                and found.json.get("ok") is True
                and len(matches) == 1
                and matches[0].get("file") == "messages.log"
                and matches[0].get("line") == 1
                and searched_line is not None
                and searched_line.group("text") == reply_text
                and searched_line.group("role") == "builder"
                and int(searched_line.group("cid")) == builder,
                f"search={found.body[:240]}",
            )

            # --- the thread and the activity window ---------------------------
            thread = lab.api_get(f"/api/v0/conversations/{CID_SAY_BUILDER}/{CID_SAY_LIBRARIAN}").json
            case.invariant(
                "the pair's thread is the two exchanges oldest first, active, with the window exported",
                [row["id"] for row in thread["messages"]] == ["c1", "c2"]
                and [row["text"] for row in thread["messages"]] == [first_text, reply_text]
                and thread["total"] == 2
                and thread["a"] == builder
                and thread["b"] == librarian
                and thread["active"] is True
                and thread["active_secs"] == ACTIVE_SECS
                and thread["truncated"] is False,
                f"{thread}",
            )
            reversed_thread = lab.api_get(f"/api/v0/conversations/{CID_SAY_LIBRARIAN}/{CID_SAY_BUILDER}").json
            case.invariant(
                "the pair is unordered: the same thread whichever way it is asked for",
                reversed_thread["messages"] == thread["messages"]
                and reversed_thread["active"] == thread["active"]
                and reversed_thread["total"] == thread["total"],
                f"reversed={reversed_thread}",
            )
            never = lab.api_get(f"/api/v0/conversations/{CID_SAY_BUILDER}/{CID_UNSERVED}").json
            case.invariant(
                "a pair that has never spoken is an empty thread, not a refusal — and not active",
                never["messages"] == []
                and never["total"] == 0
                and never["active"] is False
                and never["active_secs"] == ACTIVE_SECS,
                f"{never}",
            )
            record = lab.api_get("/api/v0/conversations").json
            case.invariant(
                "the record reads newest first, bounded, with one pair summary at the exported window",
                [row["id"] for row in record["conversations"]] == ["c2", "c1"]
                and record["total"] == 2
                and record["limit"] == CONVERSATIONS_LIMIT
                and record["truncated"] is False
                and record["active_secs"] == ACTIVE_SECS
                and record["pairs_total"] == 1
                and record["pairs_truncated"] is False
                and len(record["pairs"]) == 1
                and record["pairs"][0]["a"] == min(builder, librarian)
                and record["pairs"][0]["b"] == max(builder, librarian)
                and record["pairs"][0]["active"] is True
                and record["pairs"][0]["count"] == 2
                and record["pairs"][0]["last_at"] >= record["conversations"][0]["created_at"],
                f"{record}",
            )

            # --- the refusals -------------------------------------------------
            selfsay = lab.api_post(f"/api/v0/agents/{builder}/say", {"to_cid": builder, "text": "hello me"})
            empty = lab.api_post(f"/api/v0/agents/{builder}/say", {"to_cid": librarian, "text": "   "})
            overlong = lab.api_post(
                f"/api/v0/agents/{builder}/say", {"to_cid": librarian, "text": "x" * (MAX_SAY_TEXT + 1)}
            )
            case.invariant(
                "an agent does not speak to itself (409, word for word)",
                selfsay.status == 409
                and selfsay.json.get("error") == f"guest CID {builder} cannot speak to itself",
                f"{selfsay.status} {selfsay.body[:200]}",
            )
            case.invariant(
                "an empty say and one over the 2 KiB cap are 400s before anything is recorded",
                empty.status == 400
                and empty.json.get("error") == "text must not be empty"
                and overlong.status == 400
                and overlong.json.get("error") == f"text must be at most {MAX_SAY_TEXT} bytes",
                f"empty={empty.status} {empty.body[:120]} overlong={overlong.status} {overlong.body[:120]}",
            )
            case.invariant(
                "the say endpoint is POST-only, and a refused say changes nothing",
                lab.api_get(f"/api/v0/agents/{builder}/say").status == 405
                and lab.api_get("/api/v0/conversations").json["total"] == 2,
                f"get={lab.api_get(f'/api/v0/agents/{builder}/say').status} total={lab.api_get('/api/v0/conversations').json['total']}",
            )

            # --- a peer with no session: the delivery is recorded, not hidden --
            before = lab.api_get("/api/v0/conversations").json["total"]
            stopped = lab.admin_stop(librarian, "acceptance: the social suite ended this session")
            case.invariant(
                "POST /stop ends the peer's session",
                stopped.status == 200 and stopped.json.get("stopped") is True,
                f"{stopped.status} {stopped.body[:200]}",
            )
            case.invariant(
                "the hub's registry drains the stopped session (the peer now has no session to take a call)",
                _await_no_row(lab, librarian),
                f"row={_row_for(lab, librarian)} status={lab.status()}",
            )
            case.invariant(
                "the peer's VM is still running, so the register the node keeps still calls it a connected agent",
                any(
                    vm.get("role") == "librarian" and vm.get("state") == "running"
                    for vm in (lab.get(run_id) if run_id else {}).get("vms", [])
                ),
                f"vms={(lab.get(run_id) if run_id else {}).get('vms')}",
            )
            lost_text = f"the undeliverable one {ctx.seed}"
            lost = lab.api_post(f"/api/v0/agents/{builder}/say", {"to_cid": librarian, "text": lost_text})
            delivery = lost.json.get("delivery") if lost.status == 201 else None
            case.invariant(
                "a say the peer's session cannot take is recorded, never hidden: delivered false, with the hub's own words",
                lost.status == 201
                and lost.json.get("id") == "c3"
                and lost.json.get("text") == lost_text
                and lost.json.get("delivered") is False
                and isinstance(delivery, str)
                and delivery.startswith("the hub refused with 409: ")
                and f"guest CID {librarian} is not connected" in delivery,
                f"{lost.status} {lost.body[:240]}",
            )
            case.invariant(
                "a failed delivery is still an exchange: the record grew and the pair stays active",
                lab.api_get("/api/v0/conversations").json["total"] == before + 1
                and lab.api_get(f"/api/v0/conversations/{CID_SAY_BUILDER}/{CID_SAY_LIBRARIAN}").json["total"] == 3
                and lab.api_get(f"/api/v0/conversations/{CID_SAY_BUILDER}/{CID_SAY_LIBRARIAN}").json["active"] is True,
                f"record={lab.api_get('/api/v0/conversations').json['total']}",
            )

            _save(ctx, "say", "conversations.json", json.dumps(lab.api_get("/api/v0/conversations").json, indent=2) + "\n")
            _save(ctx, "say", "thread.json", json.dumps(lab.api_get(f"/api/v0/conversations/{CID_SAY_BUILDER}/{CID_SAY_LIBRARIAN}").json, indent=2) + "\n")
            _save(ctx, "say", "builder-messages.log", content)
            _save(ctx, "say", "say-rows.txt", "\n".join([first.body, second.body, lost.body]) + "\n")
            _save(ctx, "say", "refusals.txt", "\n".join([selfsay.body, empty.body, overlong.body, denied.body]) + "\n")
            for role in ("builder", "librarian"):
                _save(ctx, "say", f"{role}-transcript.txt", _stream_of(lab, run_id, role))

    _guarded(case, body)
    case.note(
        "the ACTIVE_SECS *decay* (a pair whose last exchange is older than the 120 s window) is NOT exercised here: "
        "nothing in the API lets a caller age an exchange and a case that slept 120 s would slow every run for a rule "
        "the store already pins. What this case asserts is what a caller can observe: the window is the exported "
        "ACTIVE_SECS (120) in every view that reports activity (so no client hard-codes it), a fresh pair is active "
        "with its count and last_at, a never-spoken pair is an empty, inactive thread, and an undelivered exchange "
        "still counts as activity. The boundary itself (inclusive) is covered by crates/noded/src/social.rs's own "
        "tests (is_active / pairs_of)."
    )


def _case_social_pass(
    suite: Suite,
    ctx,
    fleet_ok: bool,
    hubstat_ok: bool,
    admin_ok: bool,
    hubadmin_ok: bool,
    dispatchd_flags: bool,
    image: Path | None,
    image_reason: str | None,
) -> None:
    goal = "write the plaza's welcome note"
    acceptance = "the note is in the workspace"
    case = suite.case(
        "the social pass (dispatchd): the board read, the interest note, and the claim that follows",
        vector=(
            "dispatchd --once over the suite's own noded --hub-status --workspace and vsockd --admin-port: an "
            "open roled call is posted to the board by the *other* agent; the pass acts for the idle builder "
            "(the register walked cid-ascending), reads the board (its cursor advances to the newest post), "
            "posts the interest note for the open builder call (subject `available for <task>`, the exact "
            "templated body with the role's granted tools, posted_by `dispatchd`, the task link), and the claim "
            "walk that follows binds the task to the builder"
        ),
    )
    missing = [
        label
        for label, present in (
            ("the dispatchd binary", (Path(ctx.bin_dir) / "dispatchd").exists()),
            ("dispatchd --noded", dispatchd_flags),
            ("noded --agents", fleet_ok),
            ("noded --hub-status", hubstat_ok),
            ("noded --hub-admin", hubadmin_ok),
            ("vsockd --admin-port", admin_ok),
        )
        if not present
    ]
    if missing:
        case.blocked(
            "the social pass needs a binary/flag this build does not produce (probe: <binary> --help): "
            + ", ".join(missing)
            + " — so the social-pass case is SKIPPED"
        )
        return
    if image is None:
        case.blocked(
            "the social pass's claim walk drives the agent through the harness's file tools, which need the "
            f"guests' writable /workspace volume, and there is no workspace image: {image_reason} — the "
            "social-pass case is SKIPPED"
        )
        return

    def body() -> None:
        manifest = _write_manifest(ctx, "pass", {"builder": BUILDER_TOOLS, "librarian": LIBRARIAN_TOOLS})
        with Lab(
            ctx,
            [CID_PASS_BUILDER, CID_PASS_LIBRARIAN],
            "social-pass",
            agents=manifest,
            agents_cid_base=CID_PASS_BUILDER,
            roles=manifest,
            workspace=image,
            status_port=0,
            admin_port=0,
            hub_status="self",
            hub_admin="self",
        ) as lab:
            run_id, booted = _fleet_run(lab)
            case.invariant("noded booted the initial role fleet at startup", booted, lab.noded_log_text()[-400:])
            roles = _role_cids(lab, run_id) if run_id else {}
            builder = roles.get("builder")
            librarian = roles.get("librarian")
            case.invariant(
                "the two-role fleet boots one VM per role, in alphabetical order, at the CIDs the base gives them",
                builder == CID_PASS_BUILDER and librarian == CID_PASS_LIBRARIAN,
                f"roles={roles}",
            )
            connected = _sleeper(
                lambda: _connected(lab, (("builder", CID_PASS_BUILDER), ("librarian", CID_PASS_LIBRARIAN))),
                GUEST_SETTLE_S,
            )
            case.invariant("both role guests connected to the hub", connected, f"connected={_log_lines(lab, 'connected')}")
            if builder is None or librarian is None:
                return

            def board_view() -> dict:
                reply = lab.api_get("/api/v0/board")
                if reply.status != 200:
                    raise AssertionError(f"GET /api/v0/board: {reply.status} {reply.body[:200]}")
                return reply.json

            proxy = lab.api_get("/api/v0/hub")
            case.invariant(
                "noded --hub-status proxies the hub's register verbatim — the agent list the pass reads its register from",
                proxy.status == 200
                and {row.get("cid") for row in proxy.json.get("agents", [])} >= {builder, librarian},
                f"{proxy.status} {proxy.body[:240]}",
            )

            # The board's notices: an unrelated post and the notice for an open builder call, both spoken by
            # the *other* agent, so the pass has two posts to catch up on and has not already signalled.
            created = lab.api_post(
                "/api/v0/tasks", {"goal": goal, "acceptance": acceptance, "role": "builder", "budget": 400}
            )
            task_id = created.json.get("id")
            news = lab.api_post(
                "/api/v0/board", {"agent_cid": librarian, "subject": "plaza news", "body": "the floor is open"}
            )
            notice = lab.api_post(
                "/api/v0/board",
                {"agent_cid": librarian, "subject": "work going", "body": "the call is up", "task": task_id},
            )
            case.invariant(
                "the board carries a notice naming the open builder call (the pass's discovery input), newest first",
                news.status == 201
                and news.json.get("id") == "p1"
                and notice.status == 201
                and notice.json.get("id") == "p2"
                and notice.json.get("task") == task_id,
                f"{notice.status} {notice.body[:200]}",
            )
            case.invariant(
                "the builder has not read the board yet: its unread count is both notices",
                _unread(board_view(), builder) == 2,
                f"unread={board_view()['unread']}",
            )

            run = subprocess.run(
                [
                    str(ctx.bin("dispatchd")),
                    "--noded",
                    f"http://127.0.0.1:{lab.port}",
                    "--hub",
                    f"http://127.0.0.1:{lab.admin_bound}",
                    "--roles",
                    str(manifest),
                    "--once",
                ],
                capture_output=True,
                text=True,
                timeout=DISPATCHD_TIMEOUT_S,
            )
            output = run.stdout + run.stderr
            _save(ctx, "pass", "dispatchd.log", f"exit={run.returncode}\n{output}\n")
            case.invariant(
                "dispatchd --once ran exactly one pass and exited 0",
                run.returncode == 0,
                f"exit={run.returncode} {_snippet(output, 400)}",
            )
            case.invariant(
                "the pass names the agent it acted for and the board read it made (its cursor advanced past both notices)",
                f"dispatchd: social: agent cid {builder} (builder) read the board to p2; 0 unread left" in output,
                _snippet(output, 600),
            )
            board = board_view()
            case.invariant(
                "the pass advanced the acted-for agent's cursor past both notices: its only unread post is the note it then posted",
                _unread(board, builder) == 1 and board["total"] == 3,
                f"unread={board['unread']} total={board['total']}",
            )
            note = next((post for post in board["posts"] if post["agent_cid"] == builder), None)
            case.invariant(
                "the pass posted the interest note: the contract's subject, the exact templated body, the driver's name, the task link",
                note is not None
                and note["id"] == "p3"
                and note["subject"] == f"available for {task_id}"
                and note["task"] == task_id
                and note["role"] == "builder"
                and note["posted_by"] == DISPATCHD_DRIVER
                and note["body"]
                == f"builder (cid {builder}) can serve {task_id}: {goal}. acceptance: {acceptance}; tools: {COVERED_BUILDER_TOOLS}",
                f"note={note}",
            )
            case.invariant(
                "the pass posted exactly one note: the board holds both notices and the one note, newest first",
                board["total"] == 3
                and [post["id"] for post in board["posts"]] == ["p3", "p2", "p1"]
                and [post["agent_cid"] for post in board["posts"]].count(builder) == 1,
                f"posts={[(post['id'], post['agent_cid'], post['subject']) for post in board['posts']]}",
            )
            case.invariant(
                "the pass says what it did in its own log (the note's id is named against the task)",
                f"dispatchd: social: agent cid {builder} (builder) posted available for {task_id} about open task {task_id} (p3)"
                in output,
                _snippet(output, 600),
            )

            task = lab.api_get(f"/api/v0/tasks/{task_id}").json
            case.invariant(
                "the claim follows: the task is no longer open and is bound to the agent the pass spoke for",
                task.get("state") != "open"
                and task.get("agent_cid") == builder
                and task.get("assignee_id") == "builder",
                f"task={task}",
            )
            case.invariant(
                "the claim is the ledger's own transition: claiming it again is a 409",
                lab.api_post(f"/api/v0/tasks/{task_id}/claim", {"agent_cid": builder}).status == 409,
                f"{lab.api_post(f'/api/v0/tasks/{task_id}/claim', {'agent_cid': builder}).body[:200]}",
            )

            _save(ctx, "pass", "board-after.json", json.dumps(board, indent=2) + "\n")
            _save(ctx, "pass", "task-after.json", json.dumps(task, indent=2) + "\n")
            for role in ("builder", "librarian"):
                _save(ctx, "pass", f"{role}-transcript.txt", _stream_of(lab, run_id, role))

    _guarded(case, body)


# --------------------------------------------------------------------- the run


def run(ctx) -> Suite:
    suite = Suite(
        "social",
        "every case passes; a case whose required binary/flag/image is absent is BLOCKED with the reason (never a pass)",
        ctx,
    )
    reason = ctx.can_run("social")
    if reason:
        suite.blocked(reason)
        return suite
    fleet_ok = _flag(ctx, "noded", "--agents")
    hubstat_ok = _flag(ctx, "noded", "--hub-status")
    hubadmin_ok = _flag(ctx, "noded", "--hub-admin")
    admin_ok = _flag(ctx, "vsockd", "--admin-port")
    roles_ok = _flag(ctx, "vsockd", "--roles")
    dispatchd_flags = _flag(ctx, "dispatchd", "--noded") and _flag(ctx, "dispatchd", "--once")
    for label, present in (
        ("noded --agents", fleet_ok),
        ("noded --hub-status", hubstat_ok),
        ("noded --hub-admin", hubadmin_ok),
        ("vsockd --admin-port", admin_ok),
        ("vsockd --roles", roles_ok),
        ("dispatchd --noded/--once", dispatchd_flags),
    ):
        suite.note(
            f"{label}: " + ("present" if present else "ABSENT (flagged; the cases that need it are SKIPPED with the reason)")
        )
    image, image_reason = _workspace_image(ctx)
    suite.data["workspace_image"] = (
        {"path": str(image), "bytes": image.stat().st_size, "sha256": sha256_file(image)}
        if image is not None
        else {"absent": image_reason}
    )
    _case_board(suite, ctx, fleet_ok)
    _case_contacts(suite, ctx, fleet_ok)
    _case_conversations(suite, ctx, fleet_ok, admin_ok, roles_ok, hubadmin_ok, image, image_reason)
    _case_social_pass(suite, ctx, fleet_ok, hubstat_ok, admin_ok, hubadmin_ok, dispatchd_flags, image, image_reason)
    return suite


def ahash(path: Path) -> str:
    """The image's sha256 (imported late so the module's header stays small)."""
    from apz import sha256_file

    return sha256_file(path)
