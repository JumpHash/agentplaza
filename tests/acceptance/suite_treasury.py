"""Suite 6 — the treasury and the operator surfaces: budgets become real money,
and the console's hand on the fleet.

Two product slices landed after the fleet suite and this one drives them end to
end against the real daemons:

* **the treasury v0** (``crates/noded/src/treasury.rs``) — the ledger's task
  budgets are real money in an append-only double-entry journal: a budgeted
  create *mints* the budget into the task's escrow, and every terminal state
  (``done``/``failed``/``withdrawn``) *refunds* what the escrow still holds.
  Balances are derived, never stored: ``GET /api/v0/treasury`` is the accounts
  with their derived balances and the totals line, ``GET
  /api/v0/treasury/journal`` the journal itself.
* **the operator surfaces** — ``noded --hub-admin`` drives one tool call on an
  agent through the hub's admin surface (``POST /api/v0/agents/{cid}/tell``)
  and reads a task's agent's file back (``POST
  /api/v0/tasks/{id}/artifact``); ``vsockd --admin-port`` is that surface
  (``POST /tell``, ``POST /stop``) with the codex's per-role tool grants
  (``--roles``) enforced before anything is dispatched.

Four cases, each with the fixed bar the suite states:

1. **the mint/refund cycle** — a budgeted task on the suite's own daemon, its
   escrow minted, claimed and finished by a *real* one-role fleet agent, and
   refunded (the failed and the withdrawn paths too, and the unbudgeted no-op);
2. **the posting rules over the API** — what the API can reach and only that:
   the read contracts, the refusals it states, and one escrow per task. The
   daemon serves **no write-entry endpoint**, so a posting that breaks a rule
   of the schema's table is not reachable from here; the rule set itself is
   covered where it lives (``crates/noded/src/treasury.rs`` and
   ``crates/noded/tests/treasury.rs``) and this case says so rather than
   inventing an entry it cannot post;
3. **the operator surfaces end to end** — a granted tool driven through
   ``noded`` *and* through the hub directly, a role-policy 403 for a write tool
   on a role without the grant (with the guest's transcript proving the call
   never reached it), ``POST /stop`` ending a session while its VM stays up and
   the registry drains, and the artifact read-through byte-exact;
4. **an unknown / not-connected CID on every surface** — the hub's 404 (a CID
   it does not serve) and 409 (a served CID with no live session) through both
   ``noded`` and the hub's own admin surface, plus the daemon's own segment and
   method refusals.

A case whose required flag is not in the build is reported BLOCKED with the
reason (the ``--help`` probe idiom the fleet suite uses) — never silently
skipped, never counted as a pass.
"""

from __future__ import annotations

import json
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

#: The CIDs this suite serves. Each lab's CIDs are disjoint from every other
#: suite's, so a stray probe or a leaked guest can never be mistaken for one of
#: ours: the fleet suite uses 50..56 and 100..101, the demo matrix its own.
CID_CYCLE = 130
CID_OP_BUILDER = 140
CID_OP_LIBRARIAN = 141
CID_OP_IDLE = 142
CID_STUB_SERVED = 150
CID_STUB_IDLE = 151

#: A CID no lab of this suite (or any other) serves: the hub's 404. Inside the
#: transport's guest-CID range, so ``noded`` forwards it rather than refusing
#: the segment itself.
CID_UNSERVED = 999

#: The tools the codex grants each role, transcribed from
#: ``docs/agents/initial-roles.json`` for the two roles this suite runs. The
#: builder is granted the file tools; the librarian's declared tools are the
#: codex's, of which the harness implements only ``search`` — so under the
#: policy a librarian may search and may not write (``crates/vsockd/src/roles.rs``).
BUILDER_TOOLS = ["bash", "read_file", "write_file", "edit_file", "list", "search", "fetch_url"]
LIBRARIAN_TOOLS = ["read_memory", "search", "transcript_search", "read_artifact", "submit_memory_change"]

#: The tool the operator's console drives to write a file: the harness's
#: ``write_file``. Its counterpart, the artifact read-through, drives the
#: harness's ``read_file`` on the daemon's side (``noded/src/hub.rs``
#: ``READ_FILE``), so the suite only names the write.
WRITE_TOOL = "write_file"

#: The tool that takes no arguments and touches nothing: a diagnostic, always
#: driven whatever the manifest grants (``crates/agent-harness/src/tools.rs``).
UPTIME_TOOL = "uptime"

#: The journal's mint/refund memos, in the treasury's own words
#: (``crates/noded/src/treasury.rs`` ``mint_task_budget``/``refund_task_escrow``).
MINT_MEMO = "task budget"
REFUND_MEMO = "task budget refunded"

#: Who posts every entry: the schema's ``posted_by_service`` allows only service
#: names and the treasury service posts all of them.
POSTED_BY = "treasury-service"

#: The treasury's own honest note on real spend, verbatim
#: (``treasury::SPEND_NOTE``).
SPEND_NOTE = "v0 meters no real spend; a done task refunds its whole escrow"

#: The schema's ``budget_credits`` is a bigint, so a budget past ``i64::MAX`` is
#: refused at create (``crates/noded/src/tasks.rs`` ``CreateTask::checked``).
NONSENSE_BUDGET = 18446744073709551615  # u64::MAX

#: How long a log transition, a registry change or a task transition is given.
SETTLE_S = 15.0

#: How long a real role guest is given to boot, greet the hub and beat.
GUEST_SETTLE_S = 60.0

#: How long the workspace image builder may take (the budgets suite's bound).
IMAGE_BUILD_TIMEOUT_S = 120.0


# ------------------------------------------------------------- small helpers


def _flag(ctx, binary: str, needle: str) -> bool:
    """Whether ``binary``'s CLI advertises ``needle`` (a ``--help`` probe).

    The suite's gating idiom, as the fleet suite uses it: a sibling slice's flag
    can be absent from the build entirely, so a case that needs it is gated
    behind this probe rather than assuming it and failing on its absence.
    """
    try:
        out = subprocess.run(
            [str(ctx.bin(binary)), "--help"], capture_output=True, text=True, timeout=20, check=False
        )
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


def _log_tail(lab: Lab, limit: int = 400) -> str:
    return lab.vsockd_log_text()[-limit:]


def _snippet(text: str, limit: int = 200) -> str:
    return text[-limit:]


def _numeric_pair(text: str) -> bool:
    fields = text.split()
    if len(fields) < 2:
        return False
    try:
        float(fields[0]), float(fields[1])
    except ValueError:
        return False
    return True


def _row_for(lab: Lab, cid: int) -> dict | None:
    """The hub's ``/status`` registry row for ``cid``, or ``None``."""
    for row in lab.status().get("agents", []):
        if row.get("cid") == cid:
            return row
    return None


def _await_no_row(lab: Lab, cid: int, timeout: float = SETTLE_S) -> bool:
    """``cid`` has drained from the hub's registry."""
    return _sleeper(lambda: _row_for(lab, cid) is None, timeout)


def _balance(view: dict, kind: str, owner: str):
    """An account's derived balance by kind and owner, or ``None`` if unopened."""
    for account in view.get("accounts", []):
        if account.get("kind") == kind and account.get("owner_ref") == owner:
            return account.get("balance")
    return None


def _postings_sum(entry: dict) -> int:
    return sum(posting.get("amount", 0) for posting in entry.get("postings", []))


def _postings(entry: dict) -> list[int]:
    return sorted(posting.get("amount", 0) for posting in entry.get("postings", []))


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


def _workspace_image(ctx) -> tuple[Path | None, str | None]:
    """The base workspace image the file tools need, built when it is absent.

    Returns ``(image, None)`` or ``(None, reason)``. The artifact read-through
    drives the guest's own ``write_file``/``read_file`` inside its ``/workspace``
    volume; the guest's init mounts that volume only when the VMM gave it a
    block device (``scripts/mk-initrd.sh``'s ``mount_workspace``), so a
    fleet booted without ``noded --workspace`` has no volume and the file tools
    answer ``unavailable``. The builder is the repository's own
    ``scripts/mk-workspace-image.sh`` (the budgets suite's), run with the cwd at
    the repository root; when the one tool it needs (``mkfs.ext4``) is missing
    the reason says so and the operator case is BLOCKED rather than absent.
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
    the committed evidence rather than only from its own verdict: the guest
    transcripts, the API bodies and the journal land there verbatim.
    """
    path = ctx.new_dir(f"treasury-evidence-{subdir}") / name
    path.write_text(text)
    ctx.register_artifact(path)
    return path


def _write_manifest(ctx, tag: str, roles: dict[str, list[str]]) -> Path:
    """A roles manifest in this suite's work dir, readable by both daemons.

    ``noded --agents`` reads a role's name, title, permits, toolsets and budget;
    ``vsockd --roles`` reads the role's name and its granted ``tools`` (a role
    that declares none is refused). One document both can read is what makes a
    single lab a fleet *and* a policy.
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
    path = ctx.new_dir(f"treasury-manifest-{tag}") / "roles.json"
    path.write_text(json.dumps({"source": "tests/acceptance/suite_treasury.py", "roles": entries}, indent=2) + "\n")
    ctx.register_artifact(path)
    return path


# ------------------------------------------------------------------- the cases


def _case_cycle(suite: Suite, ctx, fleet_ok: bool) -> None:
    case = suite.case(
        "the mint/refund cycle over the real API (a budgeted task, a real agent, done/failed/withdrawn, and the unbudgeted no-op)",
        vector=(
            f"noded --agents <1 role builder>: a budgeted create mints its escrow (mint -B, task_escrow t1 +B, "
            f"credits==debits==B) -> a real fleet agent claims it -> finish done -> both accounts 0, the totals "
            f"doubled, the journal [1] mint [2] refund each balancing to zero with task_id/memo/posted_by; a "
            f"failed and a withdrawn task refund too; an unbudgeted task posts nothing"
        ),
    )
    if not fleet_ok:
        case.blocked(
            "noded --agents is not in this build's CLI (probe: noded --help); the role fleet has not landed, "
            "so no real agent can claim a task and the mint/refund cycle cannot run here"
        )
        return

    def body() -> None:
        manifest = _write_manifest(ctx, "cycle", {"builder": BUILDER_TOOLS})
        with Lab(
            ctx,
            [CID_CYCLE],
            "treasury-cycle",
            agents=manifest,
            agents_cid_base=CID_CYCLE,
            status_port=0,
            hub_status="off",
            hub_admin="off",
        ) as lab:
            run_id, booted = _fleet_run(lab)
            case.invariant("noded booted the initial role fleet at startup", booted, lab.noded_log_text()[-400:])
            cids = _role_cids(lab, run_id) if run_id else {}
            builder = cids.get("builder")
            case.invariant(
                "the fleet's builder VM runs as the CID the manifest gives it",
                builder == CID_CYCLE,
                f"roles={cids}",
            )
            connected = _sleeper(
                lambda: f"vsockd: agent {CID_CYCLE} role builder connected" in lab.vsockd_log_text(),
                GUEST_SETTLE_S,
            )
            case.invariant("the real builder guest connected to the hub", connected, _log_tail(lab))
            if builder is None:
                return  # the invariants above already failed; nothing to drive

            def treasury() -> dict:
                reply = lab.api_get("/api/v0/treasury")
                if reply.status != 200:
                    raise AssertionError(f"GET /api/v0/treasury: {reply.status} {reply.body[:200]}")
                return reply.json

            def journal() -> dict:
                reply = lab.api_get("/api/v0/treasury/journal")
                if reply.status != 200:
                    raise AssertionError(f"GET /api/v0/treasury/journal: {reply.status} {reply.body[:200]}")
                return reply.json

            # --- the budgeted create mints the task's escrow ------------------
            created = lab.api_post("/api/v0/tasks", {"goal": "the paid work", "budget": 750, "role": "builder"})
            case.invariant(
                "a budgeted create is a 201 whose task records the budget and is claimable",
                created.status == 201
                and created.json.get("id") == "t1"
                and created.json.get("budget") == 750
                and created.json.get("state") == "open",
                f"{created.status} {created.body[:200]}",
            )
            view = treasury()
            case.invariant(
                "the mint is debited and the task's escrow credited, as derived from the journal",
                _balance(view, "mint", "mint") == -750 and _balance(view, "task_escrow", "t1") == 750,
                f"accounts={view.get('accounts')}",
            )
            case.invariant(
                "the totals balance: credits == debits == the budget, and the sum of every balance is zero",
                view["totals"]["entries"] == 1
                and view["totals"]["credits"] == 750
                and view["totals"]["debits"] == 750
                and view["totals"]["balance"] == 0,
                f"totals={view.get('totals')}",
            )
            case.invariant(
                "v0 states its own spend honestly (nothing is metered)",
                view["spent"] == 0 and view["note"] == SPEND_NOTE,
                f"spent={view.get('spent')} note={view.get('note')!r}",
            )
            entries = journal()["entries"]
            mint = entries[0] if entries else {}
            case.invariant(
                "the journal's first entry is the mint: it balances to zero and carries task_id/root_task/memo/posted_by",
                len(entries) == 1
                and mint.get("id") == 1
                and mint.get("kind") == "mint"
                and mint.get("task_id") == "t1"
                and mint.get("root_task") == "t1"
                and mint.get("memo") == MINT_MEMO
                and mint.get("posted_by") == POSTED_BY
                and _postings_sum(mint) == 0
                and _postings(mint) == [-750, 750],
                f"entry={mint}",
            )

            # --- a real agent claims and finishes: the refund -----------------
            claimed = lab.api_post("/api/v0/tasks/t1/claim", {"agent_cid": builder})
            case.invariant(
                "a real fleet agent claims the task (its accepted role is its identity)",
                claimed.status == 200
                and claimed.json.get("state") == "claimed"
                and claimed.json.get("assignee_id") == "builder"
                and claimed.json.get("agent_cid") == builder,
                f"{claimed.status} {claimed.body[:200]}",
            )
            finished = lab.api_post("/api/v0/tasks/t1/finish", {"outcome": "done", "note": "the work is done"})
            case.invariant(
                "the agent's finish ends the task as done/accepted",
                finished.status == 200
                and finished.json.get("state") == "done"
                and finished.json.get("outcome") == "accepted",
                f"{finished.status} {finished.body[:200]}",
            )
            view = treasury()
            case.invariant(
                "the refund empties both accounts and doubles the totals",
                _balance(view, "mint", "mint") == 0
                and _balance(view, "task_escrow", "t1") == 0
                and view["totals"]["entries"] == 2
                and view["totals"]["credits"] == 1500
                and view["totals"]["debits"] == 1500
                and view["totals"]["balance"] == 0,
                f"accounts={view.get('accounts')} totals={view.get('totals')}",
            )
            entries = journal()["entries"]
            case.invariant(
                "the journal is [1] mint [2] refund, ids in order, each balancing to zero and carrying task_id/memo/posted_by",
                [entry["id"] for entry in entries] == [1, 2]
                and [entry["kind"] for entry in entries] == ["mint", "refund"]
                and entries[1]["task_id"] == "t1"
                and entries[1]["root_task"] == "t1"
                and entries[1]["memo"] == REFUND_MEMO
                and entries[1]["posted_by"] == POSTED_BY
                and all(_postings_sum(entry) == 0 for entry in entries)
                and _postings(entries[1]) == [-750, 750],
                f"entries={entries}",
            )

            # --- a failed task refunds too ------------------------------------
            failed_create = lab.api_post("/api/v0/tasks", {"goal": "the failed work", "budget": 300, "role": "builder"})
            failed_id = failed_create.json.get("id")
            lab.api_post(f"/api/v0/tasks/{failed_id}/claim", {"agent_cid": builder})
            failed = lab.api_post(f"/api/v0/tasks/{failed_id}/finish", {"outcome": "failed", "note": "gave up"})
            case.invariant(
                "a claimed task that fails ends as failed/failed and refunds its escrow",
                failed.status == 200
                and failed.json.get("state") == "failed"
                and failed.json.get("outcome") == "failed"
                and _balance(treasury(), "task_escrow", failed_id) == 0,
                f"{failed.status} {failed.body[:200]}",
            )

            # --- a withdrawn task refunds too (claimed and unclaimed) ---------
            withdrawn_create = lab.api_post("/api/v0/tasks", {"goal": "the withdrawn work", "budget": 200, "role": "builder"})
            withdrawn_id = withdrawn_create.json.get("id")
            lab.api_post(f"/api/v0/tasks/{withdrawn_id}/claim", {"agent_cid": builder})
            withdrawn = lab.api_delete(f"/api/v0/tasks/{withdrawn_id}")
            case.invariant(
                "a claimed task withdrawn mid-flight ends as withdrawn/cancelled and refunds",
                withdrawn.status == 200
                and withdrawn.json.get("state") == "withdrawn"
                and withdrawn.json.get("outcome") == "cancelled"
                and _balance(treasury(), "task_escrow", withdrawn_id) == 0,
                f"{withdrawn.status} {withdrawn.body[:200]}",
            )
            open_create = lab.api_post("/api/v0/tasks", {"goal": "never started", "budget": 120, "role": "builder"})
            open_id = open_create.json.get("id")
            open_withdrawn = lab.api_delete(f"/api/v0/tasks/{open_id}")
            case.invariant(
                "an unclaimed task withdrawn refunds its whole escrow",
                open_withdrawn.status == 200
                and open_withdrawn.json.get("state") == "withdrawn"
                and _balance(treasury(), "task_escrow", open_id) == 0,
                f"{open_withdrawn.status} {open_withdrawn.body[:200]}",
            )

            # --- an unbudgeted task posts nothing -----------------------------
            before = treasury()["totals"]["entries"]
            free_create = lab.api_post("/api/v0/tasks", {"goal": "free work"})
            free_id = free_create.json.get("id")
            case.invariant(
                "an unbudgeted create opens no escrow and posts no entry",
                free_create.status == 201
                and free_create.json.get("budget") == 0
                and _balance(treasury(), "task_escrow", free_id) is None
                and treasury()["totals"]["entries"] == before,
                f"create={free_create.body[:160]} entries={treasury()['totals']['entries']}",
            )
            lab.api_post(f"/api/v0/tasks/{free_id}/claim", {"agent_cid": builder})
            lab.api_post(f"/api/v0/tasks/{free_id}/finish", {"outcome": "done"})
            case.invariant(
                "an unbudgeted task that runs and ends still posts nothing",
                treasury()["totals"]["entries"] == before and treasury()["totals"]["balance"] == 0,
                f"entries={treasury()['totals']['entries']}",
            )

            # --- the whole journal, at the end --------------------------------
            final = treasury()
            entries = journal()["entries"]
            _save(ctx, "cycle", "treasury.json", json.dumps(final, indent=2) + "\n")
            _save(ctx, "cycle", "journal.json", json.dumps(journal(), indent=2) + "\n")
            case.invariant(
                "every entry balances to zero and the journal's ids are exactly 1..=n, in order",
                all(_postings_sum(entry) == 0 for entry in entries)
                and [entry["id"] for entry in entries] == list(range(1, len(entries) + 1)),
                f"entries={entries}",
            )
            case.invariant(
                "the journal balances as a whole and every account is back to zero",
                final["totals"]["credits"] == final["totals"]["debits"]
                and final["totals"]["balance"] == 0
                and _balance(final, "mint", "mint") == 0
                and all(account["balance"] == 0 for account in final["accounts"]),
                f"totals={final.get('totals')} accounts={final.get('accounts')}",
            )
            mints = {entry["task_id"]: entry["amount"] for entry in entries if entry["kind"] == "mint"}
            refunds = {entry["task_id"]: entry["amount"] for entry in entries if entry["kind"] == "refund"}
            case.invariant(
                "each mint is matched by a refund of the same amount for the same task (the cycle is closed)",
                mints == refunds
                and set(mints) == {"t1", failed_id, withdrawn_id, open_id}
                and mints.get("t1") == 750
                and mints.get(failed_id) == 300
                and mints.get(withdrawn_id) == 200
                and mints.get(open_id) == 120,
                f"mints={mints} refunds={refunds}",
            )

    _guarded(case, body)


def _case_posting_rules(suite: Suite, ctx) -> None:
    case = suite.case(
        "the posting rules over the API: the read contracts, the refusals the API states, and one escrow per task",
        vector=(
            "a noded with no fleet: GET /api/v0/treasury[/journal] and their read-only-ness (405/404); a nonsense "
            "budget refused 400 with the ledger unchanged; each create its own task and its own escrow (the same "
            "create twice is two tasks, never one id reused); the claim/state-machine refusals; and the rule set "
            "itself recorded as covered by the crate's own tests (the API has no write-entry endpoint)"
        ),
    )

    def body() -> None:
        # A noded alone: no vsockd, no fleet. The treasury is the daemon's own.
        with Lab(ctx, [], "treasury-rules", hub_status="off", hub_admin="off") as lab:
            initial = lab.api_get("/api/v0/treasury")
            case.invariant(
                "the treasury starts as the mint alone: one account, no entries, a zero sum",
                initial.status == 200
                and [account["kind"] for account in initial.json["accounts"]] == ["mint"]
                and initial.json["accounts"][0]["owner_ref"] == "mint"
                and initial.json["accounts"][0]["balance"] == 0
                and initial.json["totals"]["accounts"] == 1
                and initial.json["totals"]["entries"] == 0
                and initial.json["totals"]["balance"] == 0,
                f"{initial.status} {initial.body[:200]}",
            )
            case.invariant(
                "v0 states its own spend honestly (nothing is metered)",
                initial.json["spent"] == 0 and initial.json["note"] == SPEND_NOTE,
                f"spent={initial.json.get('spent')} note={initial.json.get('note')!r}",
            )
            stream = lab.api_get("/api/v0/treasury/journal")
            case.invariant(
                "the journal starts empty and untruncated",
                stream.status == 200
                and stream.json["entries"] == []
                and stream.json["total"] == 0
                and stream.json["truncated"] is False,
                f"{stream.status} {stream.body[:200]}",
            )

            # The API offers no way to post an entry: the treasury surfaces are
            # reads. A write method is refused (405), a path that is not the
            # journal is 404 — so a posting outside mint/refund is unreachable.
            writes = [
                (method, target)
                for method in ("POST", "PUT", "DELETE")
                for target in ("/api/v0/treasury", "/api/v0/treasury/journal")
            ]
            observed = {f"{method} {target}": lab.api_request(method, target, {}).status for method, target in writes}
            case.invariant(
                "the treasury endpoints accept GET only (a write method is a 405 on both)",
                all(status == 405 for status in observed.values()),
                f"{observed}",
            )
            case.invariant(
                "a path under the treasury that is not the journal is a 404",
                lab.api_get("/api/v0/treasury/nope").status == 404
                and lab.api_get("/api/v0/treasury/journal/extra").status == 404,
                f"nope={lab.api_get('/api/v0/treasury/nope').status} extra={lab.api_get('/api/v0/treasury/journal/extra').status}",
            )

            # A nonsense budget: refused, and it posts nothing and consumes no id.
            huge = lab.api_post("/api/v0/tasks", {"goal": "a nonsense budget", "budget": NONSENSE_BUDGET})
            case.invariant(
                "a budget past i64::MAX is refused 400 in the ledger's own words",
                huge.status == 400 and "budget must be at most" in huge.body,
                f"{huge.status} {huge.body[:200]}",
            )
            outside = [
                {"goal": ""},
                {"goal": "   "},
                {"goal": "x", "budget": -5},
                {"goal": "x", "budget": "lots"},
                {"goal": "x", "budget": 1.5},
            ]
            refusals = {}
            for bad in outside:
                reply = lab.api_post("/api/v0/tasks", bad)
                refusals[f"{bad}"] = reply.status
            case.invariant(
                "a create outside the contract is refused 400 (an empty goal, a non-integer budget)",
                all(status == 400 for status in refusals.values()),
                f"{refusals}",
            )
            after_refusals = lab.api_get("/api/v0/treasury").json
            case.invariant(
                "a refused create leaves the ledger and the journal exactly as they were (no id consumed)",
                lab.api_get("/api/v0/tasks").json["tasks"] == []
                and after_refusals["totals"]["entries"] == 0
                and after_refusals["totals"]["accounts"] == 1,
                f"tasks={lab.api_get('/api/v0/tasks').body[:160]} totals={after_refusals.get('totals')}",
            )

            # The same create twice is two tasks with two escrows: an id is
            # assigned once, in order, and a task's escrow is its own account.
            first = lab.api_post("/api/v0/tasks", {"goal": "the first", "budget": 100})
            second = lab.api_post("/api/v0/tasks", {"goal": "the first", "budget": 100})
            case.invariant(
                "each create is its own task: the id is handed out in order and never reused",
                first.status == 201
                and second.status == 201
                and [first.json.get("id"), second.json.get("id")] == ["t1", "t2"],
                f"first={first.body[:120]} second={second.body[:120]}",
            )
            view = lab.api_get("/api/v0/treasury").json
            case.invariant(
                "each create opens its own escrow, minted by its own entry (no shared or double-posted escrow)",
                _balance(view, "task_escrow", "t1") == 100
                and _balance(view, "task_escrow", "t2") == 100
                and view["totals"]["entries"] == 2
                and view["totals"]["credits"] == 200
                and view["totals"]["debits"] == 200
                and view["totals"]["balance"] == 0,
                f"accounts={view.get('accounts')} totals={view.get('totals')}",
            )
            entries = lab.api_get("/api/v0/treasury/journal").json["entries"]
            case.invariant(
                "each mint names its own task (the journal has one mint per create, in create order)",
                [entry["task_id"] for entry in entries] == ["t1", "t2"]
                and all(entry["kind"] == "mint" for entry in entries)
                and all(_postings_sum(entry) == 0 for entry in entries),
                f"entries={entries}",
            )

            # The ledger's own gates, over the API: a claim only from a live
            # agent, and a task's terminal state machine.
            case.invariant(
                "a claim from an agent this node has no live session for is a 409",
                lab.api_post("/api/v0/tasks/t1/claim", {"agent_cid": CID_UNSERVED}).status == 409,
                f"{lab.api_post('/api/v0/tasks/t1/claim', {'agent_cid': CID_UNSERVED}).body[:200]}",
            )
            case.invariant(
                "finishing an open task is a 409 (the ledger's state machine)",
                lab.api_post("/api/v0/tasks/t1/finish", {"outcome": "done"}).status == 409,
                f"{lab.api_post('/api/v0/tasks/t1/finish', {'outcome': 'done'}).body[:200]}",
            )

    _guarded(case, body)
    case.note(
        "the posting rules themselves are NOT exercised through the API, and this case does not invent an entry "
        "it cannot post: the daemon serves NO write-entry endpoint (the treasury surfaces are GET-only, asserted "
        "above), so a posting that breaks a rule of the schema's table is unreachable from here. The rule set is "
        "transcribed verbatim in crates/noded/src/treasury.rs (POSTING_RULES, the schema's treasury.posting_rules) "
        "and covered there by its own tests (the rule matrix, check_entry's order, the balance/overdraw/margin "
        "checks, and the mutant-killing suite), plus crates/noded/tests/treasury.rs for the API-reachable half "
        "(the mint/refund wiring, the views, the SSE frames). That is where the rest is covered; this case asserts "
        "only what this API can reach."
    )


def _case_operator(
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
        "the operator surfaces end to end: /tell (granted + the 403), /stop, and the artifact read-through",
        vector=(
            "a 2-role fleet (builder + librarian) on the suite's own vsockd --admin-port --roles and "
            "noded --agents --hub-admin: uptime driven through noded and through the hub directly; write_file "
            "granted on the builder and 403 (role librarian does not grant write_file) on the librarian, with the "
            "guest's transcript proving the call never reached it; POST /stop ends the session (the transcript "
            "shows stop -> bye -> staying up), the registry drains, the VM stays running; and the artifact "
            "read-through returns the builder's file byte-exact, with an escaping path the agent's own 422"
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
            "the operator surfaces need a flag this build's CLI does not advertise (probe: <binary> --help): "
            + ", ".join(missing)
            + " — so the end-to-end operator case is SKIPPED"
        )
        return
    if image is None:
        case.blocked(
            "the artifact read-through needs the guests' writable /workspace volume (noded --workspace) and "
            f"there is no workspace image: {image_reason} — the operator case is SKIPPED"
        )
        return

    def body() -> None:
        manifest = _write_manifest(ctx, "operator", {"builder": BUILDER_TOOLS, "librarian": LIBRARIAN_TOOLS})
        cids = [CID_OP_BUILDER, CID_OP_LIBRARIAN, CID_OP_IDLE]
        with Lab(
            ctx,
            cids,
            "treasury-operator",
            agents=manifest,
            agents_cid_base=CID_OP_BUILDER,
            roles=manifest,
            workspace=image,
            status_port=0,
            admin_port=0,
            hub_status="off",
            hub_admin="self",
        ) as lab:
            run_id, booted = _fleet_run(lab)
            case.invariant("noded booted the initial role fleet at startup", booted, lab.noded_log_text()[-400:])
            role_cids = _role_cids(lab, run_id) if run_id else {}
            builder = role_cids.get("builder")
            librarian = role_cids.get("librarian")
            case.invariant(
                "the two-role fleet boots one VM per role, in alphabetical order, at the CIDs the base gives them",
                builder == CID_OP_BUILDER and librarian == CID_OP_LIBRARIAN,
                f"roles={role_cids}",
            )
            connected = _sleeper(
                lambda: all(
                    f"vsockd: agent {cid} role {role} connected" in lab.vsockd_log_text()
                    for role, cid in (("builder", CID_OP_BUILDER), ("librarian", CID_OP_LIBRARIAN))
                ),
                GUEST_SETTLE_S,
            )
            case.invariant("both role guests connected to the hub", connected, f"connected={_log_lines(lab, 'connected')}")
            if builder is None or librarian is None:
                return

            # --- the guests' writable volume (the file tools' boundary) --------
            case.invariant(
                "each guest mounted its writable /workspace volume (noded --workspace)",
                _sleeper(
                    lambda: all(
                        "/workspace mounted" in _stream_of(lab, run_id, role) for role in ("builder", "librarian")
                    ),
                    GUEST_SETTLE_S,
                ),
                f"builder={_snippet(_stream_of(lab, run_id, 'builder'))}",
            )

            # --- the operator drives a granted tool (a diagnostic) ------------
            uptime = lab.api_post(f"/api/v0/agents/{builder}/tell", {"name": UPTIME_TOOL})
            case.invariant(
                "the operator drives uptime through noded and gets the agent's own answer",
                uptime.status == 200
                and uptime.json.get("cid") == builder
                and uptime.json.get("ok") is True
                and _numeric_pair(str(uptime.json.get("output", ""))),
                f"{uptime.status} {uptime.body[:200]}",
            )
            via_hub = lab.admin_tell(builder, UPTIME_TOOL)
            case.invariant(
                "the hub's admin surface answers the same /tell directly (the operator's own path to it)",
                via_hub.status == 200 and via_hub.json.get("ok") is True and _numeric_pair(str(via_hub.json.get("output", ""))),
                f"{via_hub.status} {via_hub.body[:200]}",
            )

            # --- the grant the role holds: write_file on the builder ----------
            content = f"acceptance-treasury artifact {ctx.seed}\nsecond line\n"
            wrote = lab.api_post(
                f"/api/v0/agents/{builder}/tell",
                {"name": WRITE_TOOL, "args": {"path": "artifact.txt", "content": content}},
            )
            case.invariant(
                "a write tool the role grants is driven and the agent answers (builder grants write_file)",
                wrote.status == 200 and wrote.json.get("cid") == builder and wrote.json.get("ok") is True,
                f"{wrote.status} {wrote.body[:200]}",
            )

            # --- the grant the role lacks: write_file on the librarian --------
            node_denied = lab.api_post(
                f"/api/v0/agents/{librarian}/tell",
                {"name": WRITE_TOOL, "args": {"path": "nope.txt", "content": "x"}},
            )
            hub_denied = lab.admin_tell(librarian, WRITE_TOOL, {"path": "nope.txt", "content": "x"})
            case.invariant(
                "a write tool on a role without the grant is the hub's own 403 (role librarian does not grant write_file)",
                node_denied.status == 403
                and hub_denied.status == 403
                and hub_denied.json.get("ok") is False
                and hub_denied.json.get("output") == "role librarian does not grant write_file",
                f"noded={node_denied.status} {node_denied.body[:200]} hub={hub_denied.status} {hub_denied.body[:200]}",
            )
            case.invariant(
                "noded carries the hub's refusal verbatim (its whole body becomes the daemon's message)",
                node_denied.json.get("error") == hub_denied.body.strip(),
                f"noded's error={node_denied.json.get('error')!r} hub's body={hub_denied.body.strip()!r}",
            )
            case.invariant(
                "the refusal was decided before dispatch: the guest never saw the call",
                _sleeper(lambda: "write_file" not in _stream_of(lab, run_id, "librarian"), SETTLE_S),
                _snippet(_stream_of(lab, run_id, "librarian")),
            )

            # --- the artifact read-through (a claimed task's agent) -----------
            created = lab.api_post(
                "/api/v0/tasks", {"goal": "the operator's artifact read", "role": "builder", "budget": 400}
            )
            task_id = created.json.get("id")
            claimed = lab.api_post(f"/api/v0/tasks/{task_id}/claim", {"agent_cid": builder})
            case.invariant(
                "the operator can claim a task for the connected builder",
                claimed.status == 200 and claimed.json.get("state") == "claimed" and claimed.json.get("agent_cid") == builder,
                f"{claimed.status} {claimed.body[:200]}",
            )
            artifact = lab.api_post(f"/api/v0/tasks/{task_id}/artifact", {"path": "artifact.txt"})
            case.invariant(
                "the read-through drives the agent's read_file and returns the file byte-exact",
                artifact.status == 200
                and artifact.json.get("task") == task_id
                and artifact.json.get("agent_cid") == builder
                and artifact.json.get("path") == "artifact.txt"
                and artifact.json.get("content") == content
                and artifact.json.get("size") == len(content.encode())
                and artifact.json.get("truncated") is False,
                f"{artifact.status} {artifact.body[:240]}",
            )
            escaped = lab.api_post(f"/api/v0/tasks/{task_id}/artifact", {"path": "../escape.txt"})
            case.invariant(
                "an escaping path is refused by the agent itself (422, its own words)",
                escaped.status == 422 and "workspace" in escaped.body,
                f"{escaped.status} {escaped.body[:200]}",
            )
            _save(ctx, "operator", "artifact-content.txt", content)
            _save(ctx, "operator", "artifact-read.json", artifact.body + "\n")
            _save(ctx, "operator", "tell-403-noded.txt", f"{node_denied.status} {node_denied.body}\n")
            _save(ctx, "operator", "tell-403-hub.txt", f"{hub_denied.status} {hub_denied.body}\n")

            # --- POST /stop: the session ends, the VM stays up ----------------
            librarian_uptime = lab.api_post(f"/api/v0/agents/{librarian}/tell", {"name": UPTIME_TOOL})
            case.invariant(
                "the librarian answers a diagnostic before the stop (its session is live)",
                librarian_uptime.status == 200 and librarian_uptime.json.get("ok") is True,
                f"{librarian_uptime.status} {librarian_uptime.body[:200]}",
            )
            reason = "acceptance: the operator stopped this session"
            stopped = lab.admin_stop(librarian, reason)
            case.invariant(
                "POST /stop ends the session, answering the guest's own call count",
                stopped.status == 200
                and stopped.json.get("cid") == librarian
                and stopped.json.get("stopped") is True
                and isinstance(stopped.json.get("calls"), int),
                f"{stopped.status} {stopped.body[:200]}",
            )
            case.invariant(
                "the hub's registry drains the stopped session",
                _await_no_row(lab, librarian),
                f"row={_row_for(lab, librarian)} status={lab.status()}",
            )
            case.invariant(
                "the guest's own transcript shows the operator's stop, its bye, and that it stays up",
                _sleeper(
                    lambda: all(
                        needle in _stream_of(lab, run_id, "librarian")
                        for needle in (
                            reason,
                            '[vsock<-] {"type":"stop"',
                            '[vsock->] {"type":"bye"',
                            "the hub stopped the agent",
                            "the guest stays up",
                            "plaza guest: agent harness stopped by the hub; staying up",
                        )
                    ),
                    GUEST_SETTLE_S,
                ),
                _snippet(_stream_of(lab, run_id, "librarian"), 400),
            )
            case.invariant(
                "the VM stays running (the register still shows the librarian's VM up)",
                any(
                    vm.get("role") == "librarian" and vm.get("state") == "running"
                    for vm in (lab.get(run_id) if run_id else {}).get("vms", [])
                ),
                f"vms={(lab.get(run_id) if run_id else {}).get('vms')}",
            )
            case.invariant(
                "the builder's session is untouched by the librarian's stop",
                lab.api_post(f"/api/v0/agents/{builder}/tell", {"name": UPTIME_TOOL}).status == 200,
                f"builder row={_row_for(lab, builder)}",
            )
            _save(ctx, "operator", "stop.json", stopped.body + "\n")
            for role in ("builder", "librarian"):
                _save(ctx, "operator", f"{role}-transcript.txt", _stream_of(lab, run_id, role))

    _guarded(case, body)


def _case_unknown_cid(suite: Suite, ctx, admin_ok: bool, hubadmin_ok: bool) -> None:
    case = suite.case(
        "an unknown or not-connected CID on every surface (the hub's 404 and 409, and the daemon's own refusals)",
        vector=(
            "vsockd serves a CID no guest uses and one it does not serve at all, and noded --hub-admin points at "
            "it: noded /tell and the hub's own /tell and /stop all answer 404 for the unserved CID and 409 for the "
            "served-but-unconnected one; the artifact read-through answers 404 for an unknown task and 409 for an "
            "unassigned one; the segment and method refusals sit in front of them"
        ),
    )
    if not (admin_ok and hubadmin_ok):
        missing = [
            label
            for label, present in (("vsockd --admin-port", admin_ok), ("noded --hub-admin", hubadmin_ok))
            if not present
        ]
        case.blocked(
            "the 404/409 matrix needs a flag this build's CLI does not advertise (probe: <binary> --help): "
            + ", ".join(missing)
            + " — so the case is SKIPPED"
        )
        return

    def body() -> None:
        # Two CIDs served, one guestless: the hub serves both, nothing connects.
        with Lab(
            ctx,
            [CID_STUB_SERVED, CID_STUB_IDLE],
            "treasury-unknown",
            admin_port=0,
            hub_status="off",
            hub_admin="self",
        ) as lab:
            served, idle = CID_STUB_SERVED, CID_STUB_IDLE

            # -- noded's own /tell: it forwards, and the hub's answer is the answer
            node_unknown = lab.api_post(f"/api/v0/agents/{CID_UNSERVED}/tell", {"name": UPTIME_TOOL})
            case.invariant(
                "noded /tell: a CID the hub does not serve is the hub's 404",
                node_unknown.status == 404 and f"no agent with guest CID {CID_UNSERVED}" in node_unknown.body,
                f"{node_unknown.status} {node_unknown.body[:200]}",
            )
            node_idle = lab.api_post(f"/api/v0/agents/{idle}/tell", {"name": UPTIME_TOOL})
            case.invariant(
                "noded /tell: a served CID with no live session is the hub's 409",
                node_idle.status == 409 and f"guest CID {idle} is not connected" in node_idle.body,
                f"{node_idle.status} {node_idle.body[:200]}",
            )
            node_bad_segment = lab.api_post("/api/v0/agents/99999999999/tell", {"name": UPTIME_TOOL})
            case.invariant(
                "noded /tell: a CID segment outside the transport's range is refused 400 before the hub",
                node_bad_segment.status == 400 and "outside" in node_bad_segment.body,
                f"{node_bad_segment.status} {node_bad_segment.body[:200]}",
            )
            node_word_segment = lab.api_post("/api/v0/agents/not-a-cid/tell", {"name": UPTIME_TOOL})
            case.invariant(
                "noded /tell: a non-numeric CID segment is refused 400",
                node_word_segment.status == 400,
                f"{node_word_segment.status} {node_word_segment.body[:200]}",
            )
            case.invariant(
                "noded /tell is POST-only (driving a tool is never advertised as a GET)",
                lab.api_get(f"/api/v0/agents/{served}/tell").status == 405,
                f"{lab.api_get(f'/api/v0/agents/{served}/tell').status}",
            )

            # -- the hub's admin surface directly
            hub_unknown = lab.admin_tell(CID_UNSERVED, UPTIME_TOOL)
            case.invariant(
                "the hub's /tell: a CID it does not serve is a 404",
                hub_unknown.status == 404 and f"no agent with guest CID {CID_UNSERVED}" in hub_unknown.body,
                f"{hub_unknown.status} {hub_unknown.body[:200]}",
            )
            hub_idle = lab.admin_tell(idle, UPTIME_TOOL)
            case.invariant(
                "the hub's /tell: a served CID with no live session is a 409",
                hub_idle.status == 409 and f"guest CID {idle} is not connected" in hub_idle.body,
                f"{hub_idle.status} {hub_idle.body[:200]}",
            )
            hub_stop_unknown = lab.admin_stop(CID_UNSERVED)
            case.invariant(
                "the hub's /stop: a CID it does not serve is a 404",
                hub_stop_unknown.status == 404 and f"no agent with guest CID {CID_UNSERVED}" in hub_stop_unknown.body,
                f"{hub_stop_unknown.status} {hub_stop_unknown.body[:200]}",
            )
            hub_stop_idle = lab.admin_stop(idle)
            case.invariant(
                "the hub's /stop: a served CID with no live session is a 409 (and nothing is torn down)",
                hub_stop_idle.status == 409 and f"guest CID {idle} is not connected" in hub_stop_idle.body,
                f"{hub_stop_idle.status} {hub_stop_idle.body[:200]}",
            )
            beyond_u32 = lab.admin_tell(4294967296, UPTIME_TOOL)
            case.invariant(
                "the hub's /tell: a CID past u32 is still an unknown (a 404, never a panic)",
                beyond_u32.status == 404,
                f"{beyond_u32.status} {beyond_u32.body[:200]}",
            )
            case.invariant(
                "the hub's admin surface accepts the POSTs only, and an unknown path is a 404",
                lab.admin_request("GET", "/tell").status == 405
                and lab.admin_request("GET", "/stop").status == 405
                and lab.admin_request("POST", "/nope", {}).status == 404,
                f"get_tell={lab.admin_request('GET', '/tell').status} "
                f"get_stop={lab.admin_request('GET', '/stop').status} "
                f"post_nope={lab.admin_request('POST', '/nope', {}).status}",
            )

            # -- the artifact read-through's own 404/409
            created = lab.api_post("/api/v0/tasks", {"goal": "an unassigned task"})
            task_id = created.json.get("id")
            unknown = lab.api_post("/api/v0/tasks/no-such-task/artifact", {"path": "artifact.txt"})
            case.invariant(
                "the artifact read-through: an unknown task is a 404",
                unknown.status == 404 and "no task with id no-such-task" in unknown.body,
                f"{unknown.status} {unknown.body[:200]}",
            )
            unassigned = lab.api_post(f"/api/v0/tasks/{task_id}/artifact", {"path": "artifact.txt"})
            case.invariant(
                "the artifact read-through: an unassigned task is a 409 (nothing to read it back from)",
                unassigned.status == 409 and "has no assignee yet" in unassigned.body,
                f"{unassigned.status} {unassigned.body[:200]}",
            )
            case.invariant(
                "the artifact read-through is POST-only",
                lab.api_get(f"/api/v0/tasks/{task_id}/artifact").status == 405,
                f"{lab.api_get(f'/api/v0/tasks/{task_id}/artifact').status}",
            )
            _save(
                ctx,
                "unknown-cid",
                "refusals.txt",
                "\n".join(
                    [
                        f"noded /tell unserved:      {node_unknown.status} {node_unknown.body}",
                        f"noded /tell served-idle:   {node_idle.status} {node_idle.body}",
                        f"noded /tell bad segment:   {node_bad_segment.status} {node_bad_segment.body}",
                        f"hub /tell unserved:        {hub_unknown.status} {hub_unknown.body}",
                        f"hub /tell served-idle:     {hub_idle.status} {hub_idle.body}",
                        f"hub /stop unserved:        {hub_stop_unknown.status} {hub_stop_unknown.body}",
                        f"hub /stop served-idle:     {hub_stop_idle.status} {hub_stop_idle.body}",
                        f"hub /tell past u32:        {beyond_u32.status} {beyond_u32.body}",
                        f"artifact unknown task:     {unknown.status} {unknown.body}",
                        f"artifact unassigned task:  {unassigned.status} {unassigned.body}",
                    ]
                )
                + "\n",
            )

    _guarded(case, body)


# --------------------------------------------------------------------- the run


def run(ctx) -> Suite:
    suite = Suite(
        "treasury",
        "every case passes; a case whose required flag/binary is absent is BLOCKED with the reason (never a pass)",
        ctx,
    )
    reason = ctx.can_run("treasury")
    if reason:
        suite.blocked(reason)
        return suite
    fleet_ok = _flag(ctx, "noded", "--agents")
    admin_ok = _flag(ctx, "vsockd", "--admin-port")
    roles_ok = _flag(ctx, "vsockd", "--roles")
    hubadmin_ok = _flag(ctx, "noded", "--hub-admin")
    for label, present in (
        ("noded --agents", fleet_ok),
        ("vsockd --admin-port", admin_ok),
        ("vsockd --roles", roles_ok),
        ("noded --hub-admin", hubadmin_ok),
    ):
        suite.note(f"{label}: " + ("present" if present else "ABSENT (flagged; the cases that need it are SKIPPED with the reason)"))
    # The operator case's artifact read-through needs the guests' writable
    # /workspace volume; resolve (and, when absent, build) its base image once.
    # Its identity goes in the suite's own data (the manifest carries the digest
    # too); the image itself is 16 MiB and is not a bounded evidence excerpt.
    image, image_reason = _workspace_image(ctx)
    suite.data["workspace_image"] = (
        {"path": str(image), "bytes": image.stat().st_size, "sha256": sha256_file(image)}
        if image is not None
        else {"absent": image_reason}
    )
    _case_cycle(suite, ctx, fleet_ok)
    _case_posting_rules(suite, ctx)
    _case_operator(suite, ctx, fleet_ok, admin_ok, roles_ok, hubadmin_ok, image, image_reason)
    _case_unknown_cid(suite, ctx, admin_ok, hubadmin_ok)
    return suite
