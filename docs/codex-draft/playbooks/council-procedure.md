# Council procedure
How the council proposes motions, votes, and routes anything above its authority to the owner. Council seat holders use it during council sessions; Mayors and departments use it to know what the council can and cannot decide.

## When to use
- You hold a council seat and a session is open, or a matter needs a motion.
- A town, budget move, Mayor replacement, law change or per-case grant needs a council decision.
- A request reaches the council step of an escalation chain (0094).

## Steps
1. Know the seats (0034). There are four: the heads of Finance, Legal (Agents) (the legislative counsel), Incident response and Mayor Office. Legal (Governance), meaning judges and prosecutors, never sits (S-008a), so courts stay independent of rule-making. Mayors bring town matters through the Mayor Office head.
2. Know what the council does: set fleet-wide priorities, move budget between towns, propose towns and laws, coordinate Mayors, and issue per-case, time-limited grants such as quarantine rights for one incident (0079). It decides through hub APIs, like Mayors; code enforces the result.
3. Check the thresholds before drafting (0067). The owner must ratify any law change, creating or dissolving a town, replacing a Mayor or department head, budget moves over 10% of a town's monthly budget, anything in the irreversible risk class, and any new tool scope for a town. Everything else the council decides by majority.
4. Check C-004 separately. Irreversible, external-facing or money-moving actions have an approval chain of the owner only (0094). The council can never approve them, and a motion to pre-approve one should be rejected or routed to the owner.
5. Draft the motion with `propose_motion` using the **council-motion** schema: `motion_id`, `proposer`, `kind` (`priority`, `budget_move`, `town_proposal`, `mayor_replacement`, `law_proposal`, `other`), `text`, and `needs_owner`. Set `needs_owner: true` whenever any 0067 threshold applies. Put the evidence and the concrete proposal in `text`.
6. If the motion affects a town, say whether its Mayor was consulted. If it bypasses the Mayor, say why.
7. Vote with `cast_vote`: `yes`, `no` or `abstain`, always with a reason. Vote on the merits and the evidence, not on department interest. Votes and reasons are logged.
8. Count. A simple majority decides. A 2–2 tie escalates to the owner (0034).
9. Below the thresholds, the decision takes effect immediately and is logged with each vote and reason. Above them, it waits for owner ratification. A law change takes effect only as an owner-signed commit on the Codex `main` branch (0049); constitutional changes are owner-only.
10. For grants, keep them narrow: one capability, one scope, the shortest duration that works. Grants expire automatically and appear in council and owner reports (0079).

## Checklist
- [ ] The matter is within the council's remit, not a single Mayor's decision.
- [ ] Every 0067 threshold was checked, and `needs_owner` is set if any applies.
- [ ] Nothing in the motion approves or pre-approves a C-004 action.
- [ ] Budget moves are compared against 10% of the affected town's monthly budget (0067).
- [ ] The motion text carries the evidence and a concrete proposal.
- [ ] Affected Mayors were consulted, or the reason for bypassing them is stated.
- [ ] Every vote has a reason tied to evidence.
- [ ] A 2–2 tie is escalated to the owner, not re-voted until it passes.
- [ ] A motion that needs ratification is not acted on before the owner ratifies.
- [ ] Grants are scoped to one case, one capability and a short duration.
- [ ] Law proposals go to legislative counsel's petition process with evidence (0049).
- [ ] No message claims an owner position the owner has not recorded (S-101c).

## Pitfalls
- Treating a majority as final on a budget move above 10% of a town's monthly budget skips ratification, so mark `needs_owner` and wait.
- Voting through a motion to pre-approve a live payment or production deploy breaks C-004, since only the owner can approve those, so vote no and route it to the owner.
- Voting yes to replace a Mayor without data is a decision without evidence, so ask for the data or vote no with a reason.
- Voting along department lines without evidence defeats the point of seats on different models, so tie every vote to the record.
- Splitting a large budget move into several small motions to stay under 10% is threshold splitting (S-104b), so propose the whole move.
- Granting broad, long-lived rights for convenience turns a per-case grant into a standing right, which 0079 rules out, so scope each grant to one incident.
- Saying "the owner wants this" without a recorded owner decision is false authority (S-101c), so cite the record or drop the claim.
- Motions that quietly override a Mayor erode town accountability, so state why the Mayor is bypassed.
