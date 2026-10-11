"""Suite 10 — the council v0, driven at its own API over a real role fleet.

``docs/agents/council.md`` is the pinned contract; ``crates/noded/src/council.rs``
is its implementation and ``crates/dispatchd/src/council.rs`` is the driver that
casts the seats' votes. This suite drives the doctrine's own cases against the
**real HTTP API** of a real ``noded`` whose initial role fleet (``noded
--agents``) is booted as six real guests — the four seats, one unseated agent
and one court role — and, for the driver case, against a real ``dispatchd``:

**(a) the seats.** Over that six-role fleet: each of the four seated roles may
propose and vote; an unseated connected agent (the builder) is refused on both
with a ``403`` naming its role **verbatim**; and the court role
``legal-gov-judge`` — which S-008a excludes by name — is refused the same way,
because judges never sit.

**(b) the derived gate.** ``needs_owner`` is derived, never trusted. A task's
budget mints a real total into the treasury's journal, and a ``budget_move`` is
owner-gated **iff ``amount * 10 > minted_total``** — asserted on both sides of
the boundary (exactly 10 % is *not* over it; one credit more is). A
caller-supplied flag that disagrees with the derivation is a ``400`` naming the
rule, in either direction: understating a gated move names the amount and the
total, understating an always-gated kind names ``0067``, and overstating a
kind the majority decides names the majority (v0 refuses routing around it).
Each always-gated kind (``law_proposal``, ``town_proposal``,
``mayor_replacement``) carries ``needs_owner: true``.

**(c) the vote rules.** A bare vote (no reason) and an unknown vote word are
``400``; an unseated voter is a ``403`` naming its role; a re-vote from the
**same seat** replaces (one vote row, the newest reason, the count unchanged);
a vote on a decided motion is a ``409`` naming the state. The tally arithmetic
is asserted after every step: ``voted == votes.length``,
``yes + no + abstain == voted``, ``seats == 4``.

**(d) the count** (0034). Four yes → ``carried``; 2–2 → ``tie_to_owner``;
3–1 → ``carried``; 3 yes + 1 abstain → the majority rule with ``voted 4`` (an
abstain counts toward the four seats having voted and never toward the
majority); a gated 4–0 → ``awaiting_owner``.

**(e) the dispatcher's council pass.** Gated on the ``dispatchd`` binary and its
flags (``--help`` probes): over its own seat fleet, an **open task of a court
role** — which the claim walk cannot drive and so leaves open — is proposed as a
single ``priority`` motion naming it, and the seats vote across passes: the word
is ``yes`` (the motion names open work) and each vote's reason is the template
over the motion's own words. The final state is ``carried``.

**(f) the SSE.** Every ``motion`` frame on ``GET /api/v0/events`` is exactly
``{id, kind, state}`` — **no text**: the motions endpoint is the only source for
a motion's words, exactly as the exchange record is for a conversation's.

**(g) the views.** The motions read is bounded (``total``/``limit``/
``truncated``, ``limit`` the endpoint's own 256); one motion reads back; an
unknown id is a ``404``; a wrong method is a ``405``.

The fleet is the suite's own (``tests/acceptance``'s pattern): fresh daemons on
ephemeral ports, a fresh work directory, and CIDs disjoint from every other
suite's (190..195 for the fleet lab and 200..204 for the driver lab; the demo
matrix uses 10..13/100..103, kill-and-resume 10..37, budgets 40, fleet
50..56/100..101, jail 60/62 and the soak 110..159, treasury 130..151, social
160..178, agora 180..183).

The seat table, the kinds, the caps and the exact refusal wording are
transcribed from the crates (each constant named where it is used) rather than
invented, so a change in the SUT that moves a bar or a word fails this suite
rather than silently re-baselining it.
"""

from __future__ import annotations

import json
import subprocess
import threading
import time
from pathlib import Path

from apz import Lab, Suite, now

#: The CIDs each lab serves. Two blocks, both disjoint from every other suite's
#: (see the module note) and from noded's own live tests (500).
CID_FLEET_BASE = 190
CID_PASS_BASE = 200

#: The council's four seats, in the doctrine's order (``crates/noded/src/
#: council.rs``'s ``SEATS``, ``crates/dispatchd/src/council.rs``'s ``SEATS``).
SEATS = ("finance", "legal-agents-counsel", "incident", "mayor-office")

#: The role that holds no seat and is booted as the *unseated connected agent*
#: (``is_seated(builder)`` is false: every seat check is exact equality).
UNSEATED = "builder"

#: The court role S-008a excludes by name: a judge never sits
#: (``crates/noded/src/council.rs``'s ``EXCLUDED_ROLES``).
COURT = "legal-gov-judge"

#: The kinds ``crates/noded/src/council.rs``'s ``always_owner_gated`` gates
#: whatever they name.
ALWAYS_GATED = ("law_proposal", "town_proposal", "mayor_replacement")

#: The kind the council's majority decides (never owner-gated): what the
#: dispatcher proposes.
COUNCIL_KIND = "priority"

#: The motions endpoint's own window (``council::MOTIONS_LIMIT``), and a motion's
#: text cap (``council::MAX_MOTION_TEXT``). Transcribed so this suite asserts the
#: numbers the daemon enforces rather than numbers of its own.
MOTIONS_LIMIT = 256
MAX_MOTION_TEXT = 4000

#: The credits the gate case mints into a task's escrow. 1000 makes the tenth a
#: whole number (100), so "exactly 10 %" is a real boundary rather than a
#: rounding artefact.
MINT = 1000

#: The roles each lab's fleet boots, and the tools each role's codex entry
#: declares (``docs/agents/initial-roles.json``). ``noded --agents`` needs the
#: role/title/permits/toolsets/budget fields; ``dispatchd --roles`` reads the
#: ``tools`` to decide what it may drive — and the judge's grants (no ``search``,
#: no write) are exactly why the claim walk leaves its task open.
FLEET_TOOLS = {
    UNSEATED: ["bash", "read_file", "write_file", "edit_file", "list", "search", "fetch_url",
               "transcript_search", "submit_claim"],
    "finance": ["bash", "read_file", "search", "read_audit", "read_usage", "file_detection",
                "submit_finance_report"],
    "incident": ["read_audit", "read_usage", "search", "request_grant", "quarantine_agent",
                 "file_detection", "submit_incident_report"],
    "legal-agents-counsel": ["read_audit", "read_case_record", "search_precedent", "read_codex",
                             "file_petition"],
    COURT: ["read_case_record", "search_precedent", "issue_ruling"],
    "mayor-office": ["read_audit", "search", "send_message", "submit_town_report"],
}
#: The driver lab's fleet: the four seats and the court role (no builder needed —
#: the driver case is about the court role the claim walk cannot touch).
PASS_TOOLS = {role: tools for role, tools in FLEET_TOOLS.items() if role != UNSEATED}

#: How long a real role guest is given to boot, greet the hub and settle.
GUEST_SETTLE_S = 90.0

#: How long a bounded waiter polls before giving up.
SETTLE_S = 15.0

#: How long one ``dispatchd --once`` pass may take (a handful of bounded loopback
#: calls plus the drive it plans).
DISPATCHD_TIMEOUT_S = 90.0

#: How many ``--once`` passes the driver case runs. Each is a fresh process whose
#: first pass is a council pass (``COUNCIL_EVERY == 3`` and ``passes % 3 == 1``),
#: and each casts at most one seat's vote. A pass reads the motions board once
#: and proposes *after* that read, so the motion it proposes is only votable on
#: the next pass: five passes are one to propose and four to vote (the four
#: seats), with the fifth pass also proving the propose step dedups.
PASS_RUNS = 5

#: How long the SSE reader keeps its socket open. It is a daemon thread; the lab
#: stopping the daemon closes the connection and ends it early, so this is only a
#: ceiling.
SSE_SECONDS = 900.0

#: A marker no legitimate motion text or vote reason may carry into an SSE frame.
#: It is planted in a motion's text so the "a frame carries no text" assertion is
#: about a string only the record knows.
SENTINEL = "SSE-TEXT-SENTINEL-4f7c"


# ------------------------------------------------------------- small helpers


def _flag(ctx, binary: str, needle: str) -> bool:
    """Whether ``binary``'s CLI advertises ``needle`` (a ``--help`` probe).

    A binary this build does not produce is ``False`` rather than a harness
    error, so a case whose gate is absent is reported BLOCKED with the reason.
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
    except Exception as err:  # noqa: BLE001 — a case must fail, not abort the run
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


def _save(ctx, subdir: str, name: str, text: str) -> Path:
    """Persist one piece of evidence and register it with the runner.

    The runner copies a bounded excerpt of every registered artifact into
    ``results/evidence/<stamp>/``, so the assertions are reproducible from the
    committed evidence rather than only from the verdict.
    """
    path = ctx.new_dir(f"council-evidence-{subdir}") / name
    path.write_text(text)
    ctx.register_artifact(path)
    return path


def _manifest(ctx, tag: str, roles: dict[str, list[str]]) -> Path:
    """A roles manifest this suite's work dir, readable by all three daemons.

    ``noded --agents`` reads the fleet's shape, ``vsockd --roles`` the codex's
    tool grants and ``dispatchd --roles`` the same grants to plan — one document
    all three can read is what makes a single lab a fleet *and* a policy (the
    social/treasury suites' idiom).
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
    path = ctx.new_dir(f"council-manifest-{tag}") / "roles.json"
    path.write_text(json.dumps({"source": "tests/acceptance/suite_council.py", "roles": entries}, indent=2) + "\n")
    ctx.register_artifact(path)
    return path


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


def _role_cids(lab: Lab, run_id: str | None) -> dict[str, int]:
    """The role each of a fleet run's VMs runs as, mapped to its guest CID."""
    view = lab.get(run_id) if run_id else {}
    return {
        vm["role"]: vm["guest_cid"]
        for vm in view.get("vms", [])
        if vm.get("role") and vm.get("guest_cid")
    }


def _connected(lab: Lab, pairs: tuple[tuple[str, int], ...]) -> bool:
    """Every named role's guest has greeted the hub, as the hub's own log says."""
    text = lab.vsockd_log_text()
    return all(f"vsockd: agent {cid} role {role} connected" in text for role, cid in pairs)


def _log_lines(lab: Lab, needle: str) -> list[str]:
    return [line for line in lab.vsockd_log_text().splitlines() if needle in line]


def _propose(lab: Lab, cid: int, kind: str, text: str, amount: int | None = None,
             needs_owner: bool | None = None):
    """``POST /api/v0/motions`` — the gate and the amount only when the caller
    names them, so "absent" and "explicitly false" stay distinct."""
    body: dict = {"proposer_cid": cid, "kind": kind, "text": text}
    if amount is not None:
        body["amount"] = amount
    if needs_owner is not None:
        body["needs_owner"] = needs_owner
    return lab.api_post("/api/v0/motions", body)


def _vote(lab: Lab, motion_id: str, cid: int, word: str, reason: str):
    return lab.api_post(
        f"/api/v0/motions/{motion_id}/vote", {"agent_cid": cid, "vote": word, "reason": reason}
    )


def _seat(role: str, cid: int) -> str:
    """The refusal the council writes for a role that holds no seat
    (``CouncilError::Unseated``'s ``Display``), word for word."""
    return f"the agent on guest CID {cid} is role {role}, which holds no council seat"


def _tally_ok(motion: dict) -> bool:
    """The tally's invariants: ``voted == votes.length``,
    ``yes + no + abstain == voted``, ``seats == 4``."""
    tally = motion.get("tally", {})
    votes = motion.get("votes", [])
    return (
        tally.get("voted") == len(votes)
        and tally.get("yes", 0) + tally.get("no", 0) + tally.get("abstain", 0) == tally.get("voted")
        and tally.get("seats") == 4
    )


def _minted_total(lab: Lab) -> int:
    """The minted budget total, read the way ``Treasury::minted_total`` derives
    it: the sum of every ``mint`` entry's positive postings, from the journal
    endpoint the daemon serves (never cached, never inferred)."""
    journal = lab.api_get("/api/v0/treasury/journal").json
    return sum(
        posting["amount"]
        for entry in journal["entries"]
        if entry["kind"] == "mint"
        for posting in entry["postings"]
        if posting["amount"] > 0
    )


def _drive_to(motion_id: str, lab: Lab, roles: dict[str, int], words: dict[str, str]) -> dict | None:
    """Cast one vote per seat in the given words and answer the final view."""
    final = None
    for seat, word in words.items():
        reply = _vote(lab, motion_id, roles[seat], word, f"the {seat} seat's reason")
        if reply.status != 200:
            return None
        final = reply.json
    return final


# ------------------------------------------------------------------- the cases

#: ``(name, vector)`` per case, fixed here so a run whose fleet gate is absent
#: reports the same case list BLOCKED instead of quietly running a shorter suite.
CASE_SEATS = (
    "(a) THE SEATS: over a six-role fleet of real guests, the four seated roles propose and vote; an "
    "unseated connected agent and a court role are refused 403 with the role named verbatim",
    "noded --agents with builder, finance, incident, legal-agents-counsel, legal-gov-judge, "
    "mayor-office: each seat's proposer_cid is accepted (201, proposer_role named) and each seat's "
    "vote is accepted (200); the builder's propose and vote are 403 `the agent on guest CID <n> is "
    "role builder, which holds no council seat`; the judge's are the same with `legal-gov-judge`",
)
CASE_GATE = (
    "(b) THE DERIVED GATE: the always-gated kinds carry needs_owner true, a budget_move is gated iff "
    "amount*10 > the minted total (the boundary both ways), and a disagreeing flag is a 400 naming the rule",
    "a 1000-credit task mints the total; a budget_move of exactly 100 is not gated, 101 is; "
    "needs_owner=false on the 101 move is 400 naming `over 10% of the minted budget total 1000`; a "
    "budget_move without an amount is 400 `must name an amount`; law_proposal/needs_owner=false is "
    "400 naming 0067; priority/needs_owner=true is 400 naming the majority; each of law_proposal, "
    "town_proposal, mayor_replacement carries needs_owner true, and each carried 4-0 waits on the "
    "owner (awaiting_owner)",
)
CASE_VOTES = (
    "(c) THE VOTE RULES: a bare vote and an unknown word are 400, an unseated voter is 403, a re-vote "
    "replaces, a vote on a decided motion is 409 naming the state, and the tally arithmetic holds",
    "one motion: `reason` absent 400, reason empty 400, vote `maybe` 400, the builder's vote 403 "
    "verbatim; finance votes no then yes — one vote row, the newest reason stands, voted unchanged at "
    "1; three more yes → carried; a further vote is 409 `motion <id> is carried, not open: a decided "
    "motion takes no further vote`; after every step voted == votes.length, yes+no+abstain == voted, "
    "seats == 4",
)
CASE_COUNT = (
    "(d) THE COUNT (0034): 4-0 carried, 2-2 tie_to_owner, 3-1 carried, 3 yes + 1 abstain carried "
    "with voted 4, and a gated 4-0 awaiting_owner",
    "five motions driven to a decided state: priority 4 yes → carried; other 2 yes 2 no → "
    "tie_to_owner; other 3 yes 1 no → carried; other 3 yes 1 abstain → carried with voted 4 and the "
    "abstain outside the majority; law_proposal (needs_owner) 4 yes → awaiting_owner",
)
CASE_SSE = (
    "(f) THE SSE: every motion frame is exactly {id, kind, state} — no text",
    "GET /api/v0/events drained across the session: every `event: motion` frame's data has exactly "
    "the keys id/kind/state; the carried motion's frames run open (on propose) then carried (on the "
    "fourth vote) — 5 frames for 1 propose + 4 votes; no frame carries the motion's own text",
)
CASE_VIEWS = (
    "(g) THE VIEWS: the bounded read (total/limit/truncated), one motion, 404 and 405",
    "GET /api/v0/motions carries total/limit/truncated with limit the endpoint's own 256; "
    "GET /api/v0/motions/{id} reads one back with the contract's fields; an unknown id is 404 `no "
    "motion with id m9999`; PUT on the collection and on one motion is 405, and GET on /vote is 405",
)
CASE_PASS = (
    "(e) THE DISPATCHER'S COUNCIL PASS: an open court task no seat can claim is proposed and the "
    "seats vote across passes",
    "dispatchd --once x5 over its own seat fleet and --roles manifest: the claim walk skips the "
    "legal-gov-judge task (the role grants no tool the dispatcher may drive), the council pass "
    "proposes one priority motion naming it (`priority: <task> — <goal>`), the seats vote yes across "
    "passes with the reason `the record: priority '<text>' names open task <task>`, the propose step "
    "dedups, and the motion ends carried 4-0",
)


def _body_seats(case, lab: Lab, roles: dict[str, int], booted: bool, connected: bool,
                expected_roles: dict[str, int]) -> None:
    def body() -> None:
        case.invariant("noded booted the initial role fleet at startup", booted, lab.noded_log_text()[-400:])
        case.invariant(
            "the six-role fleet boots one VM per role, alphabetically, at the CIDs the base gives them",
            roles == expected_roles,
            f"roles={roles} expected={expected_roles}",
        )
        case.invariant("every role guest connected to the hub", connected, f"connected={_log_lines(lab, 'connected')}")
        case.note(
            "fleet size: six roles — the four seats, the unseated builder and the court judge. The "
            "seat case needs an unseated connected agent (five roles) and the exclusion needs a court "
            "role, so the doctrine's judge rule is driven live rather than read off the seat table."
        )
        if any(role not in roles for role in (*SEATS, UNSEATED, COURT)):
            return

        # Each of the four seats may propose.
        for seat in SEATS:
            reply = _propose(lab, roles[seat], COUNCIL_KIND, f"the {seat} seat's own agenda")
            case.invariant(
                f"the {seat} seat may propose: 201 with proposer_role {seat}",
                reply.status == 201 and reply.json.get("proposer_role") == seat,
                f"{reply.status} {reply.body[:200]}",
            )

        # Each of the four seats may vote (on one motion, then it is decided).
        subject = _propose(lab, roles["finance"], "other", "a motion for the four seats to vote on")
        subject_id = subject.json.get("id")
        case.invariant(
            "a motion for the seats to vote on was recorded (201, open)",
            subject.status == 201 and subject_id and subject.json.get("state") == "open",
            f"{subject.status} {subject.body[:200]}",
        )
        for seat in SEATS:
            reply = _vote(lab, subject_id, roles[seat], "yes", f"the {seat} seat's reason")
            case.invariant(
                f"the {seat} seat may vote: 200, and the tally counts it",
                reply.status == 200 and reply.json["tally"]["voted"] >= 1 and _tally_ok(reply.json),
                f"{reply.status} {reply.body[:200]}",
            )

        # An unseated connected agent cannot propose or vote — the role is named.
        open_motion = _propose(lab, roles["finance"], "other", "a motion left open for the refusals")
        open_id = open_motion.json.get("id")
        builder = roles[UNSEATED]
        refused = _propose(lab, builder, COUNCIL_KIND, "a builder's wish")
        case.invariant(
            "an unseated connected agent (builder) cannot propose: 403 naming the role verbatim",
            refused.status == 403 and refused.json.get("error") == _seat(UNSEATED, builder),
            f"{refused.status} {refused.body[:220]}",
        )
        refused = _vote(lab, open_id, builder, "yes", "I like it")
        case.invariant(
            "an unseated connected agent (builder) cannot vote: 403 naming the role verbatim",
            refused.status == 403 and refused.json.get("error") == _seat(UNSEATED, builder),
            f"{refused.status} {refused.body[:220]}",
        )

        # The court role never sits (S-008a).
        judge = roles.get(COURT)
        if judge is None:
            case.note("no legal-gov-judge guest in the fleet: the court exclusion could not be driven live")
        else:
            refused = _propose(lab, judge, COUNCIL_KIND, "a judge's wish")
            case.invariant(
                "the court role legal-gov-judge does not sit: its propose is a 403 naming the role",
                refused.status == 403 and refused.json.get("error") == _seat(COURT, judge),
                f"{refused.status} {refused.body[:220]}",
            )
            refused = _vote(lab, open_id, judge, "yes", "I rule")
            case.invariant(
                "the court role legal-gov-judge does not sit: its vote is a 403 naming the role",
                refused.status == 403 and refused.json.get("error") == _seat(COURT, judge),
                f"{refused.status} {refused.body[:220]}",
            )

    _guarded(case, body)


def _body_gate(case, lab: Lab, roles: dict[str, int]) -> None:
    def body() -> None:
        if "finance" not in roles:
            return
        fin = roles["finance"]
        minted = lab.api_post("/api/v0/tasks", {"goal": "fund the pool", "budget": MINT})
        case.invariant(
            f"a {MINT}-credit task is created (201) and its budget mints into escrow",
            minted.status == 201,
            f"{minted.status} {minted.body[:200]}",
        )
        total = _minted_total(lab)
        case.invariant(
            f"the treasury's minted total — the journal's own mint credits — is {MINT}",
            total == MINT,
            f"minted_total={total}",
        )

        # The boundary, low side: exactly 10 % is NOT over the gate.
        low = _propose(lab, fin, "budget_move", f"move {MINT // 10} of the pool", amount=MINT // 10)
        case.invariant(
            f"a budget_move of exactly {MINT // 10} credits (10% of the minted total) is NOT owner-gated",
            low.status == 201 and low.json.get("needs_owner") is False,
            f"{low.status} {low.body[:200]}",
        )
        # The boundary, high side: one credit more IS over it.
        high = _propose(lab, fin, "budget_move", f"move {MINT // 10 + 1} of the pool", amount=MINT // 10 + 1)
        case.invariant(
            f"a budget_move of {MINT // 10 + 1} credits (one over 10%) IS owner-gated (needs_owner true)",
            high.status == 201 and high.json.get("needs_owner") is True,
            f"{high.status} {high.body[:200]}",
        )

        # A flag that disagrees with the derivation, both directions. The
        # expected messages are the SUT's own words (owner_mismatch in
        # crates/noded/src/council.rs), asserted verbatim.
        bad = _propose(lab, fin, "budget_move", "move 101 of the pool", amount=101, needs_owner=False)
        expected = (
            f"needs_owner is false, but the budget_move names 101 credits, which is over 10% of the "
            f"minted budget total {MINT}: 0067 gates it to the owner"
        )
        case.invariant(
            "a flag that understates a gated budget_move is a 400 in the rule's own words, verbatim",
            bad.status == 400 and bad.json.get("error") == expected,
            f"{bad.status} {bad.body[:260]}",
        )
        no_amount = _propose(lab, fin, "budget_move", "move some budget")
        expected = (
            "a budget_move must name an amount: 0067 gates a move over 10% of the minted budget "
            f"total, and the amount is what the rule is applied to (the minted total is {MINT})"
        )
        case.invariant(
            "a budget_move with no amount is a 400 in the rule's own words, verbatim",
            no_amount.status == 400 and no_amount.json.get("error") == expected,
            f"{no_amount.status} {no_amount.body[:260]}",
        )
        law_bad = _propose(lab, fin, "law_proposal", "a law", needs_owner=False)
        expected = (
            "needs_owner is false, but a law_proposal is always owner-gated: 0067 requires the "
            "owner to ratify it"
        )
        case.invariant(
            "a flag that understates an always-gated kind (law_proposal) is a 400, verbatim",
            law_bad.status == 400 and law_bad.json.get("error") == expected,
            f"{law_bad.status} {law_bad.body[:260]}",
        )
        over_bad = _propose(lab, fin, COUNCIL_KIND, "an agenda", needs_owner=True)
        expected = (
            "needs_owner is true, but a priority motion is below every 0067 threshold: the council "
            "decides it by majority (0034), and v0 refuses setting the flag true to route a motion "
            "around that majority"
        )
        case.invariant(
            "a flag that overstates a kind the majority decides (priority) is a 400, verbatim",
            over_bad.status == 400 and over_bad.json.get("error") == expected,
            f"{over_bad.status} {over_bad.body[:260]}",
        )

        # Each always-gated kind carries the derived flag, and keeps it when the
        # seats carry it: a 4-0 motion waits on the owner, it does not carry.
        for kind in ALWAYS_GATED:
            reply = _propose(lab, fin, kind, f"a {kind} for the plaza")
            case.invariant(
                f"a {kind} carries needs_owner true (derived, never the caller's word)",
                reply.status == 201 and reply.json.get("needs_owner") is True,
                f"{reply.status} {reply.body[:200]}",
            )
            if reply.status != 201:
                continue
            final = _drive_to(reply.json["id"], lab, roles, dict.fromkeys(SEATS, "yes"))
            case.invariant(
                f"a {kind} carried 4-0 is awaiting_owner with needs_owner still true (the owner ratifies)",
                final is not None
                and final["state"] == "awaiting_owner"
                and final["needs_owner"] is True
                and final["tally"]["yes"] == 4
                and _tally_ok(final),
                f"{final}",
            )

    _guarded(case, body)


def _body_votes(case, lab: Lab, roles: dict[str, int]) -> None:
    def body() -> None:
        if not all(role in roles for role in (*SEATS, UNSEATED)):
            return
        fin = roles["finance"]
        subject = _propose(lab, fin, "other", "a motion for the vote rules")
        motion_id = subject.json.get("id")
        case.invariant(
            "a motion to vote on was recorded (201, open)",
            subject.status == 201 and motion_id and subject.json.get("state") == "open",
            f"{subject.status} {subject.body[:200]}",
        )

        bare = lab.api_post(f"/api/v0/motions/{motion_id}/vote", {"agent_cid": fin, "vote": "yes"})
        case.invariant(
            "a bare vote (no `reason`) is a 400: the deserializer names the missing field",
            bare.status == 400
            and str(bare.json.get("error", "")).startswith("invalid vote request: missing field `reason`"),
            f"{bare.status} {bare.body[:200]}",
        )
        empty = _vote(lab, motion_id, fin, "yes", "")
        case.invariant(
            "a vote with an empty reason is a 400 in the contract's own words, verbatim",
            empty.status == 400 and empty.json.get("error") == "reason must not be empty",
            f"{empty.status} {empty.body[:200]}",
        )
        unknown = _vote(lab, motion_id, fin, "maybe", "sound")
        case.invariant(
            "an unknown vote word is a 400 naming the enum's own membership (yes|no|abstain)",
            unknown.status == 400
            and "invalid vote request: unknown variant `maybe`, expected one of `yes`, `no`, `abstain`"
            in unknown.json.get("error", ""),
            f"{unknown.status} {unknown.body[:220]}",
        )
        unseated = _vote(lab, motion_id, roles[UNSEATED], "yes", "I like it")
        case.invariant(
            "an unseated voter is a 403 naming its role verbatim",
            unseated.status == 403 and unseated.json.get("error") == _seat(UNSEATED, roles[UNSEATED]),
            f"{unseated.status} {unseated.body[:220]}",
        )
        case.invariant(
            "no refused vote touched the motion: it is still open with an empty tally",
            lab.api_get(f"/api/v0/motions/{motion_id}").json["tally"]
            == {"yes": 0, "no": 0, "abstain": 0, "voted": 0, "seats": 4},
            lab.api_get(f"/api/v0/motions/{motion_id}").body[:200],
        )

        # One vote per seat: a later vote from the SAME seat replaces.
        first = _vote(lab, motion_id, fin, "no", "I want the ledger first")
        case.invariant(
            "the first vote is recorded and the tally counts it",
            first.status == 200 and first.json["tally"]["voted"] == 1 and _tally_ok(first.json),
            f"{first.status} {first.body[:200]}",
        )
        replaced = _vote(lab, motion_id, fin, "yes", "on reflection the dashboard is urgent")
        case.invariant(
            "a re-vote from the same seat replaces: one vote row, the newest reason stands",
            replaced.status == 200
            and len(replaced.json["votes"]) == 1
            and replaced.json["votes"][0]["reason"] == "on reflection the dashboard is urgent"
            and replaced.json["votes"][0]["vote"] == "yes",
            f"{replaced.status} {replaced.body[:240]}",
        )
        case.invariant(
            "the re-vote did not change the count: voted stays 1, no seat is double-counted",
            replaced.json["tally"] == {"yes": 1, "no": 0, "abstain": 0, "voted": 1, "seats": 4},
            f"{replaced.json['tally']}",
        )
        case.invariant(
            "the tally arithmetic holds after each step (voted == votes.length, yes+no+abstain == voted, seats 4)",
            _tally_ok(replaced.json),
            f"{replaced.json['tally']}",
        )

        # Decide it, then a vote on the decided motion.
        final = _drive_to(motion_id, lab, roles, {"legal-agents-counsel": "yes", "incident": "yes",
                                                  "mayor-office": "yes"})
        case.invariant(
            "the motion is carried after four yes votes (a majority of yes over no)",
            final is not None and final.get("state") == "carried" and _tally_ok(final),
            f"{final}",
        )
        late = _vote(lab, motion_id, fin, "yes", "again")
        case.invariant(
            "a vote on a decided motion is a 409 naming the state",
            late.status == 409
            and late.json.get("error") == f"motion {motion_id} is carried, not open: a decided motion takes no further vote",
            f"{late.status} {late.body[:240]}",
        )

    _guarded(case, body)


def _body_count(case, lab: Lab, roles: dict[str, int], observed: dict) -> None:
    def body() -> None:
        if not all(role in roles for role in SEATS):
            return

        def decided(kind: str, words: dict[str, str], text: str, amount: int | None = None) -> dict | None:
            motion = _propose(lab, roles["finance"], kind, text, amount=amount)
            if motion.status != 201:
                return None
            return _drive_to(motion.json["id"], lab, roles, words)

        carried = decided(COUNCIL_KIND, dict.fromkeys(SEATS, "yes"), f"a priority for the fleet — {SENTINEL}")
        case.invariant(
            "driving all four seats to yes carries an ungated motion",
            carried is not None and carried["state"] == "carried" and carried["needs_owner"] is False,
            f"{carried}",
        )
        case.invariant(
            "the carried motion's tally is 4 yes, 0 no, 0 abstain, voted 4 of 4 seats",
            carried is not None and _tally_ok(carried)
            and {k: carried["tally"][k] for k in ("yes", "no", "abstain", "voted", "seats")}
            == {"yes": 4, "no": 0, "abstain": 0, "voted": 4, "seats": 4},
            f"{carried['tally'] if carried else None}",
        )
        # The SSE case reads this motion's frames.
        observed["carried_id"] = carried["id"] if carried else None
        observed["carried_kind"] = COUNCIL_KIND

        tied = decided(
            "other",
            {"finance": "yes", "legal-agents-counsel": "yes", "incident": "no", "mayor-office": "no"},
            "a motion the towns disagree on",
        )
        case.invariant(
            "an even split (2-2) is tie_to_owner: the owner decides, never re-voted until it passes",
            tied is not None and tied["state"] == "tie_to_owner" and tied["tally"]["yes"] == 2
            and tied["tally"]["no"] == 2 and tied["needs_owner"] is False,
            f"{tied}",
        )

        majority = decided(
            "other",
            {"finance": "yes", "legal-agents-counsel": "yes", "incident": "yes", "mayor-office": "no"},
            "a motion with one dissent",
        )
        case.invariant(
            "3 yes to 1 no is carried (a majority of yes over no)",
            majority is not None and majority["state"] == "carried" and majority["tally"]["yes"] == 3
            and majority["tally"]["no"] == 1 and _tally_ok(majority),
            f"{majority}",
        )

        abstained = decided(
            "other",
            {"finance": "yes", "legal-agents-counsel": "yes", "incident": "yes", "mayor-office": "abstain"},
            "a motion with one abstention",
        )
        case.invariant(
            "3 yes and 1 abstain is carried: the abstain counts toward voted (4) and never toward the majority",
            abstained is not None and abstained["state"] == "carried" and abstained["tally"]["yes"] == 3
            and abstained["tally"]["abstain"] == 1 and abstained["tally"]["voted"] == 4
            and _tally_ok(abstained),
            f"{abstained}",
        )

        gated = decided(
            "law_proposal",
            dict.fromkeys(SEATS, "yes"),
            "a law the owner must ratify",
        )
        case.invariant(
            "a gated motion carried 4-0 is awaiting_owner with needs_owner still true",
            gated is not None and gated["state"] == "awaiting_owner" and gated["needs_owner"] is True
            and gated["tally"]["yes"] == 4 and _tally_ok(gated),
            f"{gated}",
        )

    _guarded(case, body)


def _body_views(case, lab: Lab, observed: dict) -> None:
    def body() -> None:
        listing = lab.api_get("/api/v0/motions")
        view = listing.json
        case.invariant(
            "the motions read is bounded: total/limit/truncated, limit the endpoint's own 256",
            listing.status == 200
            and view.get("limit") == MOTIONS_LIMIT
            and isinstance(view.get("total"), int)
            and view.get("truncated") is False
            and len(view.get("motions", [])) == view.get("total"),
            f"{view.get('total')} total, limit {view.get('limit')}, {len(view.get('motions', []))} in the window",
        )
        first_id = observed.get("carried_id") or (view["motions"][0]["id"] if view.get("motions") else None)
        one = lab.api_get(f"/api/v0/motions/{first_id}") if first_id else None
        case.invariant(
            "one motion reads back by id with the contract's fields",
            one is not None
            and one.status == 200
            and {
                "id", "proposer_cid", "proposer_role", "kind", "text", "needs_owner",
                "state", "votes", "tally", "created_at", "updated_at",
            } <= set(one.json),
            f"{one.status if one else None} {one.body[:200] if one else ''}",
        )
        missing = lab.api_get("/api/v0/motions/m9999")
        case.invariant(
            "an unknown motion id is a 404 in the endpoint's own words",
            missing.status == 404 and missing.json.get("error") == "no motion with id m9999",
            f"{missing.status} {missing.body[:200]}",
        )
        put = lab.api_request("PUT", "/api/v0/motions", {})
        case.invariant(
            "a wrong method on the collection is a 405 (the motions read is a GET)",
            put.status == 405,
            f"{put.status} {put.body[:200]}",
        )
        put_one = lab.api_request("PUT", f"/api/v0/motions/{first_id}", {})
        get_vote = lab.api_get(f"/api/v0/motions/{first_id}/vote") if first_id else None
        case.invariant(
            "a wrong method on one motion is a 405, and a vote is a POST (a GET on /vote is a 405)",
            put_one.status == 405 and get_vote is not None and get_vote.status == 405,
            f"PUT={put_one.status} GET /vote={get_vote.status if get_vote else None}",
        )

    _guarded(case, body)


def _parse_motion_frames(lines: list[str]) -> list[dict]:
    """Every ``event: motion`` frame's data object, in the order received.

    SSE frames are an ``event:`` line and a ``data:`` line; the reader keeps
    every line, so pairs are recovered by walking with the last event name.
    """
    frames: list[dict] = []
    pending: str | None = None
    for raw in lines:
        line = raw.rstrip("\r\n")
        if line.startswith("event: "):
            pending = line[len("event: "):].strip()
        elif line.startswith("data: ") and pending == "motion":
            try:
                frames.append(json.loads(line[len("data: "):]))
            except json.JSONDecodeError:
                frames.append({"malformed": line})
            pending = None
    return frames


def _body_sse(case, frames: list[str], observed: dict) -> None:
    def body() -> None:
        carried_id = observed.get("carried_id")
        carried_kind = observed.get("carried_kind", COUNCIL_KIND)
        # Wait until the stream has caught up with the carried motion's final
        # frame (the reader thread appends to the sink as frames arrive).
        if carried_id:
            _sleeper(
                lambda: any(
                    f["id"] == carried_id and f["state"] == "carried"
                    for f in _parse_motion_frames(list(frames))
                    if "id" in f
                ),
                SETTLE_S,
            )
        # One snapshot: the reader thread keeps appending, and iterating the
        # sink while it grows is a race.
        lines = list(frames)
        motion_frames = [f for f in _parse_motion_frames(lines) if "malformed" not in f]
        case.invariant(
            "the stream carried motion frames at all (the reader saw the session's events)",
            len(motion_frames) > 0,
            f"{len(motion_frames)} motion frames",
        )
        case.invariant(
            "every motion frame is exactly {id, kind, state} — no text, no tally, no reasons",
            bool(motion_frames) and all(sorted(f) == ["id", "kind", "state"] for f in motion_frames),
            f"sample={motion_frames[:2]} bad={[f for f in motion_frames if sorted(f) != ['id','kind','state']][:3]}",
        )
        if carried_id:
            mine = [f for f in motion_frames if f.get("id") == carried_id]
            states = [f["state"] for f in mine]
            case.invariant(
                "the carried motion has a frame on propose (open) and on each of its four votes (open x3, carried)",
                states == ["open", "open", "open", "open", "carried"]
                and all(f["kind"] == carried_kind for f in mine),
                f"states={states}",
            )
        sentinel_lines = [line for line in lines if line.startswith("data:") and SENTINEL in line]
        case.invariant(
            "no motion frame carries the motion's own text (the motions endpoint is the only source)",
            not sentinel_lines,
            f"sentinel present: {sentinel_lines[:2]}",
        )

    _guarded(case, body)


def _case_pass(suite: Suite, ctx, fleet_ok: bool, dispatchd_ok: bool, hubstat_ok: bool,
               admin_ok: bool) -> None:
    case = suite.case(CASE_PASS[0], vector=CASE_PASS[1])
    missing = [
        label
        for label, present in (
            ("noded --agents", fleet_ok),
            ("the dispatchd binary", (Path(ctx.bin_dir) / "dispatchd").exists()),
            ("dispatchd --noded/--once", dispatchd_ok),
            ("noded --hub-status", hubstat_ok),
            ("vsockd --admin-port", admin_ok),
        )
        if not present
    ]
    if missing:
        case.blocked(
            "the dispatcher's council pass needs a binary/flag this build does not produce (probe: "
            "<binary> --help): " + ", ".join(missing) + " — the council-pass case cannot run here"
        )
        return

    def body() -> None:
        manifest = _manifest(ctx, "pass", PASS_TOOLS)
        cids = [CID_PASS_BASE + i for i in range(len(PASS_TOOLS))]
        # `noded` boots its fleet in the manifest's alphabetical role order.
        expected_roles = {role: CID_PASS_BASE + i for i, role in enumerate(sorted(PASS_TOOLS))}
        with Lab(
            ctx,
            cids,
            "council-pass",
            agents=manifest,
            agents_cid_base=CID_PASS_BASE,
            roles=manifest,
            status_port=0,
            admin_port=0,
            hub_status="self",
            hub_admin="self",
        ) as lab:
            run_id, booted = _fleet_run(lab)
            roles = _role_cids(lab, run_id) if run_id else {}
            case.invariant("noded booted the seat fleet at startup", booted, lab.noded_log_text()[-400:])
            case.invariant(
                "the five-role fleet (four seats + the court) boots at the CIDs the base gives them",
                roles == expected_roles,
                f"roles={roles} expected={expected_roles}",
            )
            connected = _sleeper(
                lambda: _connected(lab, tuple((role, cid) for role, cid in expected_roles.items())),
                GUEST_SETTLE_S,
            )
            case.invariant("every role guest connected to the hub", connected, f"connected={_log_lines(lab, 'connected')}")
            if not all(role in roles for role in SEATS) or COURT not in roles:
                return

            # An open task of a court role: the driver's claim walk cannot drive
            # it (no grant covers any script), so it stays open by design.
            goal = "rule on the plaza's petition"
            created = lab.api_post(
                "/api/v0/tasks",
                {"goal": goal, "acceptance": "a ruling is filed", "role": COURT, "budget": 0},
            )
            task_id = created.json.get("id")
            case.invariant(
                "an open task of the court role exists (the driver cannot claim it by design)",
                created.status == 201 and created.json.get("state") == "open" and created.json.get("role") == COURT,
                f"{created.status} {created.body[:200]}",
            )

            outputs: list[str] = []
            for index in range(PASS_RUNS):
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
                outputs.append(run.stdout + run.stderr)
                case.invariant(
                    f"dispatchd --once pass {index + 1} exited 0",
                    run.returncode == 0,
                    f"exit={run.returncode} {(run.stdout + run.stderr)[:300]}",
                )
            output = "\n".join(outputs)
            _save(ctx, "pass", "dispatchd.log", output)

            case.invariant(
                "the claim walk leaves the court task open: the role grants no tool this dispatcher may drive",
                f"dispatchd: task {task_id} (role {COURT}): the role grants no tool this dispatcher may drive; skipped"
                in output,
                [line for line in output.splitlines() if task_id in line][:3],
            )
            case.invariant(
                "the ledger still shows the court task open after every pass",
                lab.api_get(f"/api/v0/tasks/{task_id}").json.get("state") == "open",
                lab.api_get(f"/api/v0/tasks/{task_id}").body[:200],
            )

            proposal_text = f"{COUNCIL_KIND}: {task_id} — {goal}"
            listing = lab.api_get("/api/v0/motions").json
            naming = [motion for motion in listing["motions"] if task_id in motion.get("text", "")]
            case.invariant(
                "the council pass proposed exactly one motion naming the open court task (the proposal is the task's own words)",
                len(naming) == 1
                and naming[0]["kind"] == COUNCIL_KIND
                and naming[0]["text"] == proposal_text
                and naming[0]["proposer_role"] == SEATS[0],
                f"{[(m['id'], m['kind'], m['text'], m['proposer_role']) for m in naming]}",
            )
            case.invariant(
                "the propose step dedups by whole-token naming: the later passes propose nothing again",
                sum(1 for line in output.splitlines() if "dispatchd: council:" in line and " proposed " in line) == 1,
                [line for line in output.splitlines() if " proposed " in line][:4],
            )

            motion_id = naming[0]["id"] if naming else None
            expected_reason = f"the record: {COUNCIL_KIND} '{proposal_text}' names open task {task_id}"
            vote_lines = [
                line for line in output.splitlines()
                if "dispatchd: council:" in line and " voted " in line
            ]
            case.invariant(
                "the seats voted yes across passes (the motion names open work), one seat per pass",
                len(vote_lines) == 4 and all(" voted yes on motion " in line for line in vote_lines),
                vote_lines[:4],
            )
            case.invariant(
                "each vote's reason is the template over the motion's own text (the motion's own words cited)",
                bool(vote_lines) and all(line.endswith(expected_reason) for line in vote_lines),
                vote_lines[:2],
            )
            if motion_id:
                final = lab.api_get(f"/api/v0/motions/{motion_id}").json
                _save(ctx, "pass", "motion.json", json.dumps(final, indent=2) + "\n")
                case.invariant(
                    "the motion carried 4-0: every seat voted yes and priority is not owner-gated",
                    final["state"] == "carried"
                    and final["needs_owner"] is False
                    and final["tally"] == {"yes": 4, "no": 0, "abstain": 0, "voted": 4, "seats": 4}
                    and _tally_ok(final),
                    f"{final}",
                )
                case.invariant(
                    "the votes' seats are exactly the four seats (one vote each, the newest reason standing)",
                    sorted(vote["seat"] for vote in final["votes"]) == sorted(SEATS),
                    f"{[vote['seat'] for vote in final['votes']]}",
                )
            case.note(
                "what the driver case could not drive live: v0 has no owner-ratification surface (the "
                "state `awaiting_owner` is what the motion waits for), no quorum/session windows and no "
                "law signing — each is its own later slice (docs/agents/council.md), so nothing here "
                "pretends the owner acted."
            )
            _save(ctx, "pass", "tasks.json", json.dumps(lab.api_get("/api/v0/tasks").json, indent=2) + "\n")

    _guarded(case, body)


# --------------------------------------------------------------------- the run


def run(ctx) -> Suite:
    suite = Suite(
        "council",
        "every case passes: only the four seats propose and vote, the gate is derived and refused when "
        "disagreed with, the vote rules and the count follow the doctrine, the dispatcher's council pass "
        "drives the seats, and every SSE motion frame is exactly {id,kind,state}",
        ctx,
    )
    reason = ctx.can_run("council")
    if reason:
        suite.blocked(reason)
        return suite

    fleet_ok = _flag(ctx, "noded", "--agents") and _flag(ctx, "noded", "--agents-cid-base")
    dispatchd_ok = _flag(ctx, "dispatchd", "--noded") and _flag(ctx, "dispatchd", "--once")
    hubstat_ok = _flag(ctx, "noded", "--hub-status")
    hubadmin_ok = _flag(ctx, "noded", "--hub-admin")
    admin_ok = _flag(ctx, "vsockd", "--admin-port")
    for label, present in (
        ("noded --agents", fleet_ok),
        ("noded --hub-status", hubstat_ok),
        ("noded --hub-admin", hubadmin_ok),
        ("vsockd --admin-port", admin_ok),
        ("dispatchd --noded/--once", dispatchd_ok),
    ):
        suite.note(
            f"{label}: " + ("present" if present else "ABSENT (flagged; the cases that need it are BLOCKED with the reason)")
        )

    case_seats = suite.case(CASE_SEATS[0], vector=CASE_SEATS[1])
    case_gate = suite.case(CASE_GATE[0], vector=CASE_GATE[1])
    case_votes = suite.case(CASE_VOTES[0], vector=CASE_VOTES[1])
    case_count = suite.case(CASE_COUNT[0], vector=CASE_COUNT[1])
    case_views = suite.case(CASE_VIEWS[0], vector=CASE_VIEWS[1])
    case_sse = suite.case(CASE_SSE[0], vector=CASE_SSE[1])

    if not fleet_ok:
        fleet_reason = (
            "the council cases need a real role fleet and `noded --agents` is not in this build's CLI "
            "(probe: noded --help), so no real agent can be a seat"
        )
        for case in (case_seats, case_gate, case_votes, case_count, case_views, case_sse):
            case.blocked(fleet_reason)
    else:
        manifest = _manifest(ctx, "fleet", FLEET_TOOLS)
        expected_roles = {role: CID_FLEET_BASE + i for i, role in enumerate(sorted(FLEET_TOOLS))}
        cids = [CID_FLEET_BASE + i for i in range(len(FLEET_TOOLS))]
        frames: list[str] = []
        observed: dict = {}
        with Lab(
            ctx,
            cids,
            "council-fleet",
            agents=manifest,
            agents_cid_base=CID_FLEET_BASE,
            hub_status="off",
            hub_admin="off",
        ) as lab:
            run_id, booted = _fleet_run(lab)
            roles = _role_cids(lab, run_id) if run_id else {}
            connected = _sleeper(
                lambda: _connected(lab, tuple((role, cid) for role, cid in expected_roles.items())),
                GUEST_SETTLE_S,
            )
            # The SSE reader spans the whole session, so the motion frames are
            # captured as they happen rather than inferred (a daemon thread; the
            # lab stopping the daemon closes the connection and ends it).
            threading.Thread(
                target=lambda: lab.stream_events(SSE_SECONDS, frames), daemon=True
            ).start()
            _body_seats(case_seats, lab, roles, booted, connected, expected_roles)
            _body_gate(case_gate, lab, roles)
            _body_votes(case_votes, lab, roles)
            _body_count(case_count, lab, roles, observed)
            _body_views(case_views, lab, observed)
            _body_sse(case_sse, frames, observed)
            _save(ctx, "fleet", "motions.json", json.dumps(lab.api_get("/api/v0/motions").json, indent=2) + "\n")
            _save(ctx, "fleet", "sse-frames.txt", "".join(line + "\n" for line in frames))
            suite.data["fleet"] = {
                "run": run_id,
                "roles": roles,
                "motions": len(lab.api_get("/api/v0/motions").json["motions"]),
            }

    _case_pass(suite, ctx, fleet_ok, dispatchd_ok, hubstat_ok, admin_ok)
    return suite
