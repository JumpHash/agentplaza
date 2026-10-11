"""Suite 8 — the agora: the market the plaza's roled tasks offer, and the hiring
integration that ties an award to the board and to the ledger's own escrow.

`crates/noded/src/agora.rs` is the implementation and `docs/agents/social.md`
the pinned contract for the *hiring* half ("a post may name a `task`; the
agora's award posts a hiring note automatically … so the board is where
discovery happens and the agora is where it concludes"). Two cases:

1. **the market end to end** — a roled open task *is* a call (its capability,
  `spec_ref` and `max_price` derived from the task, never stored beside it); a
  connected agent of the call's own role bids and a re-bid replaces its own row;
  the award binds the task through the ledger's **own claim** (an awarded task
  is indistinguishable from a claimed one afterwards); and the award's social
  integration is asserted where it lands: the call reads back `awarded` with the
  award's row, the award's `escrow_entry` names the mint the treasury journal
  actually holds for the task, and the board carries the hiring note with the
  contract's subject and the template the task's own fields compose.
2. **the market's refusals and the bounded read** — every bid and award refusal
  the endpoints state, in their own words; the unbudgeted call awards with a
  `null` escrow entry and posts no journal entry; and the calls response is
  bounded exactly as the journal's and the board's are
  (`total`/`limit`/`truncated`), at 257 calls.

The split with `suite_social`: **that** suite owns the plaza's *talking* (the
board, the cursors, the contacts, the conversations, the social pass). This one
owns the *market* — the calls, the bids, the award, and the hiring integration
the award performs (the board's note and the escrow entry). The contact links an
award also seeds are a *social* fact and are asserted there, from the same
vector; this suite asserts only the market and the board/journal side.

The labs are this suite's own (fresh daemons, ephemeral ports, a fresh work
directory, CIDs disjoint from every other suite's: 180..183 here).
"""

from __future__ import annotations

import json
import subprocess
import time
from pathlib import Path

from apz import (
    Lab,
    Suite,
    now,
    reconstruct_console,
)

#: The CIDs this suite serves, one block per lab, disjoint from every other
#: suite's (the fleet suite uses 50..56/100..101, the treasury suite 130..151,
#: the social suite 160..178).
CID_FLOW_BUILDER = 180
CID_FLOW_LIBRARIAN = 181
CID_RULES_BUILDER = 182
CID_RULES_LIBRARIAN = 183

#: A CID no lab of this suite serves: the register's "no live agent" refusal.
CID_UNSERVED = 999

#: The tools the codex grants each role this suite runs, transcribed from
#: ``docs/agents/initial-roles.json`` (the treasury and social suites'
#: transcription, for the same roles).
BUILDER_TOOLS = ["bash", "read_file", "write_file", "edit_file", "list", "search", "fetch_url"]
LIBRARIAN_TOOLS = ["read_memory", "search", "transcript_search", "read_artifact", "submit_memory_change"]

#: The bound the agora states for a calls *response*, transcribed from
#: ``crates/noded/src/agora.rs`` ``AGORA_LIMIT`` (the same line the journal and
#: the board carry).
AGORA_LIMIT = 256

#: The journal's mint memo, in the treasury's own words (``treasury.rs``
#: ``mint_task_budget``), so the escrow the award names is checked to be the
#: mint and not merely an id.
MINT_MEMO = "task budget"

#: How long a real role guest is given to boot, greet the hub and beat.
GUEST_SETTLE_S = 60.0


# ------------------------------------------------------------- small helpers


def _flag(ctx, binary: str, needle: str) -> bool:
    """Whether ``binary``'s CLI advertises ``needle`` (a ``--help`` probe)."""
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
    head = line.split("noded: initial role fleet ", 1)[1]
    return head.split(":", 1)[0], True


def _connected(lab: Lab, pairs: tuple[tuple[str, int], ...]) -> bool:
    """Every named role's guest has greeted the hub, as the hub's log says."""
    text = lab.vsockd_log_text()
    return all(f"vsockd: agent {cid} role {role} connected" in text for role, cid in pairs)


def _save(ctx, subdir: str, name: str, text: str) -> Path:
    """Persist one piece of evidence and register it with the runner."""
    path = ctx.new_dir(f"agora-evidence-{subdir}") / name
    path.write_text(text)
    ctx.register_artifact(path)
    return path


def _write_manifest(ctx, tag: str, roles: dict[str, list[str]]) -> Path:
    """A roles manifest both daemons can read (the treasury/social idiom)."""
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
    path = ctx.new_dir(f"agora-manifest-{tag}") / "roles.json"
    path.write_text(json.dumps({"source": "tests/acceptance/suite_agora.py", "roles": entries}, indent=2) + "\n")
    ctx.register_artifact(path)
    return path


def _postings(entry: dict) -> list[int]:
    return sorted(posting.get("amount", 0) for posting in entry.get("postings", []))


# ------------------------------------------------------------------- the cases


def _case_market_flow(suite: Suite, ctx, fleet_ok: bool) -> None:
    goal = "the plaza's paid work"
    acceptance = "the artifact is filed"
    case = suite.case(
        "the market end to end: the call, the bid, the award that binds through the claim, and the hiring integration",
        vector=(
            "noded --agents <builder + librarian>: a roled open task is a call (capability/spec_ref/max_price "
            "derived from the task); the builder bids and a re-bid replaces its own row; the award binds the "
            "task through the ledger's own claim (a later claim is the ledger's 409); the call reads back "
            "`awarded` at the price the agent bid; the award's `escrow_entry` names the task's budget mint, "
            "which the treasury journal actually holds; and the board carries the hiring note — subject "
            "`hiring for <task>`, the template over the task's own fields, posted by the awarder"
        ),
    )
    if not fleet_ok:
        case.blocked(
            "noded --agents is not in this build's CLI (probe: noded --help); the role fleet has not landed, "
            "so no real agent can bid or be awarded and the market case cannot run here"
        )
        return

    def body() -> None:
        manifest = _write_manifest(ctx, "flow", {"builder": BUILDER_TOOLS, "librarian": LIBRARIAN_TOOLS})
        with Lab(
            ctx,
            [CID_FLOW_BUILDER, CID_FLOW_LIBRARIAN],
            "agora-flow",
            agents=manifest,
            agents_cid_base=CID_FLOW_BUILDER,
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
                builder == CID_FLOW_BUILDER and librarian == CID_FLOW_LIBRARIAN,
                f"roles={roles}",
            )
            connected = _sleeper(
                lambda: _connected(lab, (("builder", CID_FLOW_BUILDER), ("librarian", CID_FLOW_LIBRARIAN))),
                GUEST_SETTLE_S,
            )
            case.invariant("both role guests connected to the hub", connected, f"connected={_log_lines(lab, 'connected')}")
            if builder is None or librarian is None:
                return

            def agora() -> dict:
                reply = lab.api_get("/api/v0/agora")
                if reply.status != 200:
                    raise AssertionError(f"GET /api/v0/agora: {reply.status} {reply.body[:200]}")
                return reply.json

            # --- a roled open task is a call -----------------------------------
            created = lab.api_post(
                "/api/v0/tasks",
                {"goal": goal, "acceptance": acceptance, "role": "builder", "budget": 500},
            )
            task_id = created.json.get("id")
            board = agora()
            case.invariant(
                "a roled open task is a call: capability, spec_ref and max_price derived from the task, no bids, no award",
                created.status == 201
                and board["total"] == 1
                and board["limit"] == AGORA_LIMIT
                and board["truncated"] is False
                and len(board["calls"]) == 1
                and board["calls"][0]["task"] == task_id
                and board["calls"][0]["capability"] == "builder"
                and board["calls"][0]["spec_ref"] == acceptance
                and board["calls"][0]["max_price"] == 500
                and board["calls"][0]["state"] == "open"
                and board["calls"][0]["bids"] == []
                and board["calls"][0]["award"] is None
                and board["calls"][0]["created_at"] > 1_700_000_000_000,
                f"{created.status} {board}",
            )

            # --- a connected agent of the role bids ---------------------------
            bid = lab.api_post(f"/api/v0/tasks/{task_id}/bid", {"agent_cid": builder, "price": 300})
            case.invariant(
                "a connected agent of the call's own role bids: the row names its call, bidder and price",
                bid.status == 200
                and bid.json.get("task") == task_id
                and bid.json.get("agent_cid") == builder
                and bid.json.get("bidder") == "builder"
                and bid.json.get("price") == 300,
                f"{bid.status} {bid.body[:200]}",
            )
            rebid = lab.api_post(f"/api/v0/tasks/{task_id}/bid", {"agent_cid": builder, "price": 350})
            case.invariant(
                "a re-bid replaces the agent's own row: the call still holds one bid, at the new price",
                rebid.status == 200
                and rebid.json.get("price") == 350
                and [row["price"] for row in agora()["calls"][0]["bids"]] == [350],
                f"{rebid.status} {rebid.body[:200]} bids={agora()['calls'][0]['bids']}",
            )

            # --- the award binds through the ledger's own claim ---------------
            awarded = lab.api_post(
                f"/api/v0/tasks/{task_id}/award", {"agent_cid": builder, "awarder_cid": librarian}
            )
            case.invariant(
                "the award binds the task through the ledger's own claim: claimed, assignee_id and agent_cid set",
                awarded.status == 200
                and awarded.json.get("state") == "claimed"
                and awarded.json.get("assignee_id") == "builder"
                and awarded.json.get("agent_cid") == builder,
                f"{awarded.status} {awarded.body[:200]}",
            )
            case.invariant(
                "an awarded task is indistinguishable from a claimed one: claiming it again is the ledger's own 409",
                lab.api_post(f"/api/v0/tasks/{task_id}/claim", {"agent_cid": builder}).status == 409,
                f"{lab.api_post(f'/api/v0/tasks/{task_id}/claim', {'agent_cid': builder}).body[:200]}",
            )
            call = agora()["calls"][0]
            entry = call.get("award")
            case.invariant(
                "the call reads back `awarded`, with the award's row at the price the agent bid (never one the caller named)",
                call["state"] == "awarded"
                and entry is not None
                and entry.get("task") == task_id
                and entry.get("contractor") == "builder"
                and entry.get("agent_cid") == builder
                and entry.get("price") == 350
                and entry.get("created_at", 0) > 1_700_000_000_000,
                f"call={call}",
            )

            # --- the escrow linkage into the treasury journal ------------------
            journal = lab.api_get("/api/v0/treasury/journal").json
            mint = journal["entries"][0] if journal.get("entries") else {}
            case.invariant(
                "the award's `escrow_entry` names the ledger's own escrow: the task's budget mint, which the treasury journal holds",
                entry is not None
                and entry.get("escrow_entry") is not None
                and entry.get("escrow_entry") == mint.get("id")
                and mint.get("kind") == "mint"
                and mint.get("task_id") == task_id
                and mint.get("memo") == MINT_MEMO
                and _postings(mint) == [-500, 500],
                f"escrow_entry={entry.get('escrow_entry') if entry else None} mint={mint}",
            )

            # --- the hiring note on the board ---------------------------------
            posts = lab.api_get("/api/v0/board").json
            note = next(
                (post for post in posts.get("posts", []) if post.get("task") == task_id and post["subject"].startswith("hiring for")),
                None,
            )
            case.invariant(
                "the award posts the board's hiring note: the contract's subject, the awarder as author, the task link",
                posts["total"] == 1
                and note is not None
                and note["subject"] == f"hiring for {task_id}"
                and note["agent_cid"] == librarian
                and note["role"] == "librarian"
                and note["posted_by"] == "librarian"
                and note["task"] == task_id,
                f"note={note}",
            )
            case.invariant(
                "the hiring note's body is the template over the task's own fields (capability — goal, and its acceptance)",
                note is not None and note["body"] == f"builder — {goal} (acceptance: {acceptance})",
                f"body={note['body'] if note else None!r}",
            )

            _save(ctx, "flow", "agora-after-award.json", json.dumps(agora(), indent=2) + "\n")
            _save(ctx, "flow", "journal.json", json.dumps(journal, indent=2) + "\n")
            _save(ctx, "flow", "board-note.json", json.dumps(note, indent=2) + "\n")
            for role in ("builder", "librarian"):
                _save(ctx, "flow", f"{role}-transcript.txt", _stream_of(lab, run_id, role))

    _guarded(case, body)


def _case_market_rules(suite: Suite, ctx, fleet_ok: bool) -> None:
    case = suite.case(
        "the market's refusals, the unbudgeted call's null escrow, and the bounded calls read",
        vector=(
            "noded --agents <builder + librarian>: every bid and award refusal in the endpoint's own words (a "
            "role that is not the call's capability, a price above the ceiling, an unconnected agent, a task "
            "that is not a call, an award with no bid, an unknown task, a malformed body, the method "
            "contract); a closed call takes neither a bid nor a second award; an unbudgeted call awards with a "
            "`null` escrow entry and posts no journal entry; and the calls response is bounded at 257 calls "
            "(the newest 256, oldest first, untruncated window said so)"
        ),
    )
    if not fleet_ok:
        case.blocked(
            "noded --agents is not in this build's CLI (probe: noded --help); the role fleet has not landed, "
            "so no real agent can bid and the market's refusals cannot be driven here"
        )
        return

    def body() -> None:
        manifest = _write_manifest(ctx, "rules", {"builder": BUILDER_TOOLS, "librarian": LIBRARIAN_TOOLS})
        with Lab(
            ctx,
            [CID_RULES_BUILDER, CID_RULES_LIBRARIAN],
            "agora-rules",
            agents=manifest,
            agents_cid_base=CID_RULES_BUILDER,
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
                builder == CID_RULES_BUILDER and librarian == CID_RULES_LIBRARIAN,
                f"roles={roles}",
            )
            connected = _sleeper(
                lambda: _connected(lab, (("builder", CID_RULES_BUILDER), ("librarian", CID_RULES_LIBRARIAN))),
                GUEST_SETTLE_S,
            )
            case.invariant("both role guests connected to the hub", connected, f"connected={_log_lines(lab, 'connected')}")
            if builder is None or librarian is None:
                return

            def agora() -> dict:
                reply = lab.api_get("/api/v0/agora")
                if reply.status != 200:
                    raise AssertionError(f"GET /api/v0/agora: {reply.status} {reply.body[:200]}")
                return reply.json

            call = lab.api_post(
                "/api/v0/tasks", {"goal": "the call under test", "role": "builder", "budget": 500}
            ).json.get("id")
            plain = lab.api_post("/api/v0/tasks", {"goal": "not a call: no role"}).json.get("id")

            # --- every refusal, in the endpoint's own words --------------------
            wrong_role = lab.api_post(f"/api/v0/tasks/{call}/bid", {"agent_cid": librarian, "price": 10})
            above = lab.api_post(f"/api/v0/tasks/{call}/bid", {"agent_cid": builder, "price": 600})
            offline = lab.api_post(f"/api/v0/tasks/{call}/bid", {"agent_cid": CID_UNSERVED, "price": 10})
            unroled = lab.api_post(f"/api/v0/tasks/{plain}/bid", {"agent_cid": builder, "price": 10})
            no_bid = lab.api_post(f"/api/v0/tasks/{call}/award", {"agent_cid": builder})
            award_unroled = lab.api_post(f"/api/v0/tasks/{plain}/award", {"agent_cid": builder})
            award_offline = lab.api_post(
                f"/api/v0/tasks/{call}/award", {"agent_cid": builder, "awarder_cid": CID_UNSERVED}
            )
            unknown_bid = lab.api_post("/api/v0/tasks/t404/bid", {"agent_cid": builder, "price": 10})
            unknown_award = lab.api_post("/api/v0/tasks/t404/award", {"agent_cid": builder})
            malformed = lab.api_post(f"/api/v0/tasks/{call}/bid", {"agent_cid": "not a cid", "price": 10})
            case.invariant(
                "a bid from an agent that is not the call's capability is a 409, naming both roles",
                wrong_role.status == 409
                and wrong_role.json.get("error") == f"the agent on guest CID {librarian} is role librarian, not builder",
                f"{wrong_role.status} {wrong_role.body[:200]}",
            )
            case.invariant(
                "a bid above the call's ceiling is a 409, naming the price and the ceiling",
                above.status == 409 and above.json.get("error") == "bid price 600 exceeds max_price 500",
                f"{above.status} {above.body[:200]}",
            )
            case.invariant(
                "a bid from an agent this node has no live session for is the register's own 409",
                offline.status == 409
                and offline.json.get("error") == f"no live agent has guest CID {CID_UNSERVED}",
                f"{offline.status} {offline.body[:200]}",
            )
            case.invariant(
                "a bid on a task that names no role is a 409: there is no call there",
                unroled.status == 409
                and unroled.json.get("error") == f"task {plain} names no role, so it is not a call",
                f"{unroled.status} {unroled.body[:200]}",
            )
            case.invariant(
                "an award to an agent with no bid on the call is a 409, naming both",
                no_bid.status == 409
                and no_bid.json.get("error") == f"award to guest CID {builder} matches no bid on call {call}",
                f"{no_bid.status} {no_bid.body[:200]}",
            )
            case.invariant(
                "an award on a task that names no role is a 409: there is no call there",
                award_unroled.status == 409
                and award_unroled.json.get("error") == f"task {plain} names no role, so it is not a call",
                f"{award_unroled.status} {award_unroled.body[:200]}",
            )
            case.invariant(
                "an awarder the daemon has no live session for is checked before anything moves: 409",
                award_offline.status == 409
                and award_offline.json.get("error") == f"no live agent has guest CID {CID_UNSERVED}",
                f"{award_offline.status} {award_offline.body[:200]}",
            )
            case.invariant(
                "a bid or an award for an unknown task is a 404, in the ledger's own words",
                unknown_bid.status == 404
                and unknown_bid.json.get("error") == "no task with id t404"
                and unknown_award.status == 404
                and unknown_award.json.get("error") == "no task with id t404",
                f"bid={unknown_bid.status} {unknown_bid.body[:160]} award={unknown_award.status} {unknown_award.body[:160]}",
            )
            case.invariant(
                "a malformed bid body is a 400 before anything is consulted",
                malformed.status == 400 and "invalid bid request" in malformed.body,
                f"{malformed.status} {malformed.body[:200]}",
            )
            case.invariant(
                "the bid and the award are POST-only",
                lab.api_get(f"/api/v0/tasks/{call}/bid").status == 405
                and lab.api_get(f"/api/v0/tasks/{call}/award").status == 405,
                f"bid={lab.api_get(f'/api/v0/tasks/{call}/bid').status} award={lab.api_get(f'/api/v0/tasks/{call}/award').status}",
            )
            unchanged = agora()
            case.invariant(
                "a refused bid or award changes nothing: the call still holds no bid and no award",
                unchanged["total"] == 1
                and unchanged["calls"][0]["bids"] == []
                and unchanged["calls"][0]["award"] is None
                and unchanged["calls"][0]["state"] == "open",
                f"{unchanged}",
            )

            # --- a closed call takes neither a bid nor a second award ----------
            lab.api_post(f"/api/v0/tasks/{call}/bid", {"agent_cid": builder, "price": 300})
            closed = lab.api_post(
                f"/api/v0/tasks/{call}/award", {"agent_cid": builder, "awarder_cid": librarian}
            )
            twice = lab.api_post(f"/api/v0/tasks/{call}/award", {"agent_cid": builder})
            late_bid = lab.api_post(f"/api/v0/tasks/{call}/bid", {"agent_cid": builder, "price": 400})
            case.invariant(
                "the award closes the call and binds the task",
                closed.status == 200 and closed.json.get("state") == "claimed",
                f"{closed.status} {closed.body[:200]}",
            )
            case.invariant(
                "a second award on the same call is a 409 naming the contractor it already has",
                twice.status == 409
                and twice.json.get("error") == f"call {call} is already awarded to builder",
                f"{twice.status} {twice.body[:200]}",
            )
            case.invariant(
                "a bid on a call that has closed is a 409 naming the state it is in",
                late_bid.status == 409 and late_bid.json.get("error") == f"call {call} is claimed, not open",
                f"{late_bid.status} {late_bid.body[:200]}",
            )

            # --- an unbudgeted call: a null escrow, and no journal entry -------
            entries_before = len(lab.api_get("/api/v0/treasury/journal").json["entries"])
            free = lab.api_post("/api/v0/tasks", {"goal": "the free call", "role": "builder"}).json.get("id")
            free_bid = lab.api_post(f"/api/v0/tasks/{free}/bid", {"agent_cid": builder, "price": 0})
            free_award = lab.api_post(
                f"/api/v0/tasks/{free}/award", {"agent_cid": builder, "awarder_cid": librarian}
            )
            free_call = next((row for row in agora()["calls"] if row["task"] == free), None)
            case.invariant(
                "an unbudgeted call bids at zero and awards with a null escrow entry (there is no mint to name)",
                free_bid.status == 200
                and free_bid.json.get("price") == 0
                and free_award.status == 200
                and free_call is not None
                and free_call["max_price"] == 0
                and free_call["state"] == "awarded"
                and free_call["award"] is not None
                and free_call["award"]["escrow_entry"] is None
                and free_call["award"]["price"] == 0,
                f"bid={free_bid.status} award={free_award.status} call={free_call}",
            )
            case.invariant(
                "an unbudgeted call posts no journal entry (nothing was minted)",
                len(lab.api_get("/api/v0/treasury/journal").json["entries"]) == entries_before,
                f"entries={len(lab.api_get('/api/v0/treasury/journal').json['entries'])} before={entries_before}",
            )

            # --- the calls response is bounded ---------------------------------
            while agora()["total"] < AGORA_LIMIT + 1:
                lab.api_post("/api/v0/tasks", {"goal": f"a call {agora()['total']}", "role": "builder"})
            ledger = lab.api_get("/api/v0/tasks").json["tasks"]
            roled = [task["id"] for task in ledger if task.get("role")]
            board = agora()
            case.invariant(
                "the calls response is bounded: 257 calls, the newest 256 returned in ledger order, truncated said so",
                board["total"] == 257
                and len(board["calls"]) == AGORA_LIMIT
                and board["truncated"] is True
                and board["limit"] == AGORA_LIMIT
                and [row["task"] for row in board["calls"]] == roled[1:],
                f"total={board['total']} len={len(board['calls'])} truncated={board['truncated']} "
                f"first={board['calls'][0]['task']} last={board['calls'][-1]['task']} roled={len(roled)}",
            )

            _save(ctx, "rules", "refusals.txt", "\n".join(
                [
                    f"bid, wrong role:      {wrong_role.status} {wrong_role.body}",
                    f"bid, above ceiling:   {above.status} {above.body}",
                    f"bid, unconnected:     {offline.status} {offline.body}",
                    f"bid, not a call:      {unroled.status} {unroled.body}",
                    f"award, no bid:        {no_bid.status} {no_bid.body}",
                    f"award, not a call:    {award_unroled.status} {award_unroled.body}",
                    f"award, unconnected awarder: {award_offline.status} {award_offline.body}",
                    f"bid, unknown task:    {unknown_bid.status} {unknown_bid.body}",
                    f"bid, malformed body:  {malformed.status} {malformed.body}",
                    f"second award:         {twice.status} {twice.body}",
                    f"bid on a closed call: {late_bid.status} {late_bid.body}",
                ]
            ) + "\n")
            _save(ctx, "rules", "calls-257.json", json.dumps(board, indent=2) + "\n")

    _guarded(case, body)


# --------------------------------------------------------------------- the run


def run(ctx) -> Suite:
    suite = Suite(
        "agora",
        "every case passes; a case whose required flag is absent is BLOCKED with the reason (never a pass)",
        ctx,
    )
    reason = ctx.can_run("agora")
    if reason:
        suite.blocked(reason)
        return suite
    fleet_ok = _flag(ctx, "noded", "--agents")
    suite.note(
        "noded --agents: "
        + ("present" if fleet_ok else "ABSENT (flagged; the cases that need it are SKIPPED with the reason)")
    )
    _case_market_flow(suite, ctx, fleet_ok)
    _case_market_rules(suite, ctx, fleet_ok)
    return suite
