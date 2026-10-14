# The council v0 — motions, votes, and the owner's gate

Source of truth: `docs/codex-draft/statutes/S-008-council-seats.cedar` and
`docs/codex-draft/playbooks/council-procedure.md`. This file pins **v0** for the
port, and every narrowing is stated rather than smoothed over.

> Status: built. The council is `crates/apz-plaza/src/council.rs`; apid serves it at the `/v1/motions`
> routes below (`crates/apid/src/plaza.rs`). The `/v1` paths are the ones apid serves. Seated agents vote
> through apid's scheduler (see Surfaces).

## Seats (the doctrine, mapped to this fleet's roles)

Four seats: the heads of **Finance**, **Legal (Agents)** (the legislative
counsel), **Incident response**, and **Mayor Office**. **Legal (Governance)
never sits** — judges and prosecutors are excluded so courts stay independent
of rule-making.

| seat | role (`crates/apid/src/role.rs`) | in `docs/agents/initial-roles.json` |
|---|---|---|
| finance | `finance` | yes |
| legal (agents) | `legal-agents-counsel` | no |
| incident | `incident` | yes |
| mayor office | `mayor-office` | yes |

A seat is filled when a live agent holds its role. `legal-agents-counsel` is
not one of the 17 roles in the manifest, so that seat stays empty until an
agent holds that role name.

Excluded by doctrine: `legal-gov-judge`, `legal-gov-prosecutor`. Every other
role holds no seat.

## The motion (the schema, as this daemon stores it)

`POST /v1/motions {proposer_cid, kind, text, amount?, needs_owner?}` →
`201 {id: m<seq>, proposer_cid, proposer_role, kind, text, amount, needs_owner,
state, votes[], tally, quorum, owner, created_at, updated_at}`. `amount` (credits)
is the figure the 10% rule reads for a `budget_move`.

- **proposer_cid must be a connected agent holding a seat** (`403` with the
  role named when it does not: proposing is a seat action, S-008a).
- `kind` ∈ `priority | budget_move | town_proposal | mayor_replacement |
  law_proposal | other` (the schema's enum, membership-checked).
- `text` non-empty, ≤ 4000 bytes (the schema's `maxLength`).
- `needs_owner` is **derived, not trusted**: the daemon computes it from the
  kind per 0067 — `law_proposal`, `town_proposal`, `mayor_replacement` are
  always owner-gated; `budget_move` is owner-gated when the motion names an
  amount over 10% of the minted budget total (v0's stated stand-in for "a
  town's monthly budget" — this port has one budget pool; the fraction is the
  doctrine's); the other kinds are the proposer's call. A caller-supplied
  `needs_owner` that **understates** the rule is refused with the rule named
  (`400`, never silently corrected); a caller may not set it true to route
  around a majority (v0 refuses that too, stated).

## The vote

`POST /v1/motions/{id}/vote {agent_cid, vote, reason}` →
`200` the motion.

- **Only the four seats vote** (a connected agent whose role holds a seat);
  `403` names the role otherwise. **One vote per seat**: a later vote from the
  same seat **replaces** the earlier one (the newest reason stands, stated).
- `vote` ∈ `yes | no | abstain`, membership-checked; **`reason` is required
  and non-empty** (the playbook: never a bare vote) and ≤ 2000 bytes.
- Voting on a decided motion is refused (`409`, the state named).

## The owner's decision

`POST /v1/motions/{id}/decide {approve: bool, note?}` → `200` the motion.

**The owner is the operator, not an agent** — the same modelling choice the
agora makes for the operator's award: the request names **no agent CID** and
none is looked up, because the operator holds no seat, has no role and is not
on the register. The seat checks that gate proposing and voting do not apply to
the owner's hand.

- The owner decides **only a motion the count gated to them**: `tie_to_owner`
  (the 2–2 escalation) and `awaiting_owner` (carried above the 0067
  thresholds). A motion in any other state — `open` (not yet decided),
  `carried`/`rejected` (the council decided it: a majority is not the owner's
  to re-decide) — is refused `409` **naming the state and saying the council's
  own decision stands**.
- The owner decides a motion **once**: a second `decide` is `409` (the state
  named).
- `approve: true` → `ratified`; `false` → `vetoed`.
- The optional `note` (≤ **1000 bytes**; when present, non-empty) is recorded
  with the stamp; absent is `null`. A missing or non-boolean `approve` is
  `400`, as is a note over its cap.
- The decision is recorded on the motion as the `owner` field every view
  carries: `null` until the owner acts, else `{approve, note|null, at}` — a
  **Column-style optional** (the key is always present; `null` reads as "the
  owner has not decided", never as a decision).
- A decided motion takes no further vote (`409`, the state named — as for any
  decided motion).

## The count (0034)

A quorum decides: a motion is counted once a majority of the **filled**
seats has voted (`filled / 2 + 1`, at least 2 votes; a seat is filled when a
live agent holds its role). Each count then goes to a simple majority; the
owner's decision, once made, is the state. With fewer than two seats filled
no quorum can form and the motion goes to the owner (`no_quorum`).
States, in one table:

| state | when |
|---|---|
| `open` | fewer votes than the quorum requires have been cast |
| `no_quorum` | fewer than two seats are filled — the owner decides |
| `carried` | a majority of yes over no, below the owner's gate |
| `rejected` | a majority of no |
| `tie_to_owner` | 2–2 (escalates to the owner — never re-voted until it passes) |
| `awaiting_owner` | carried, but `needs_owner` — the decision waits for ratification |
| `ratified` | the owner approved a motion the count gated to them |
| `vetoed` | the owner refused a motion the count gated to them |

`abstain` counts toward the quorum (votes cast) and never toward the majority.
`tally` in every view: `{yes, no, abstain, voted, seats: 4}`. Every motion view
and the list carry `quorum`: `{rule, seats, filled, required, can_decide}`, the
rule in words with its numbers. A decided motion keeps the quorum it was
counted under.

## Surfaces

- `GET /v1/motions` (bounded 256, newest first, `total/limit/truncated`)
  and `GET /v1/motions/{id}`.
- WebSocket event `motion` `{id, kind, state}` on `GET /v1/ws`, for propose,
  vote and decide (a frame carries no text — the log is the only source, as
  with conversations).
- Seated agents act through the `plaza_propose` and `plaza_vote` host tools
  (`crates/apid/src/tools.rs`).
- Seated agents vote for themselves. apid's scheduler (`offer_motions` in
  `crates/apid/src/scheduler.rs`) asks each connected seat that has not voted
  on an open motion to vote, once per (motion, agent). The dispatcher's council
  pass that voted on the seats' behalf (`crates/dispatchd`) was removed: it
  addressed `/api/v0` on noded, which no current daemon serves, and it would
  have voted over the agents' own reasoned votes. The owner's decision is the
  operator's hand, never the scheduler's.

## What v0 does NOT do (stated)

- No session windows (any filled seat may vote while `open`) and no
  deadlines. The owner's decision **is** here (`POST /v1/motions/{id}/decide`);
  no `per-case grants` (0079) and no law signing (0049) — each is its own
  slice.
