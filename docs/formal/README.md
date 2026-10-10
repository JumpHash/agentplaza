# Formal models (TLA+)

Five specs: AgentSession, Approval, Economy, TaskLifecycle, RestoreMerge; 28 mutants and 16 witnesses. Model-checked with TLC (tla2tools 1.8.0). Run: `TLA2TOOLS=/path/to/tla2tools.jar ./check.sh`.
`check.sh` checks each spec, then regenerates the **mutants** (deliberately broken copies, `make_mutants.py`) and requires TLC to catch every one. A spec whose mutants survive has invariants that are too weak.

## AgentSession.tla: one agent across VMs and machines

Covers tool calls authorised and recorded by the orchestrator (0084), snapshots at tool boundaries and background upload (0043, 0091), machine failure and return, the owner's `MoveRun` (0102), restore after a kill (0027), the Hello/resume handshake (0084) and fencing (0102).

| Invariant | Meaning |
|---|---|
| `AtMostOneAccepted` | Never two copies of the agent that the orchestrator would accept |
| `NoDoubleEffect` | No tool call's external effect happens twice |
| `NoStaleEffect` | No external effect comes from a stale session, or for a call already closed |
| `OnlyCurrentSessionWrites` | The transcript is only written by the session current at that moment |
| `TranscriptWellFormed` | ≤ 1 decision and ≤ 1 result per call; results follow decisions |
| `HarnessIsPrefix` | A harness never believes in entries the orchestrator doesn't have |

Bounds: 2 machines, 2 tool calls, 3 VMs. **4,144,576 distinct states, no violation.**

## Approval.tla: escalation chains (0094)

| Invariant | Meaning |
|---|---|
| `DecidedByChain` | Only the authority at the current step can decide; late or out-of-turn answers are rejected |
| `C004OnlyOwner` | C-004 requests are only ever approved by the owner |
| `ExpiredIsNotApproved` | Silence never means yes |
| `FinalStays` (temporal) | Once decided or expired, a request never changes |

## Economy.tla: escrow, subcontracts, payment, clawback, fines (0039, 0109–0112)

One task chain root → s1 → s2 (the depth limit), two contractors, a 30-credit budget, spends of 5, prices 10 or 20, a 20% cap and a credit line of 1. Every step is one balanced ledger entry, mirroring [treasury-draft/schema.sql](../treasury-draft/schema.sql).

| Invariant | Meaning |
|---|---|
| `Conserved` | All accounts (mint, Treasury, external spend, escrows, earnings) always sum to zero |
| `EscrowNeverNegative` | No task escrow overdraws |
| `EarningsAboveCreditLine` | Earnings never go below the agent's credit line, even after clawbacks and fines |
| `NoDoublePayment` | Each task is paid at most once |
| `FailedIsNeverPaid` | Failed work earns no margin |
| `MarginCap` | Total margin in the chain ≤ 20% of money that really left (0111) |
| `ExternalIsMeteredSpend` | Real money leaves only as metered spend from escrow |
| `EarningsFromWorkOnly` | Earnings = margins paid − fines; nothing else creates them |
| `TreasuryFromSanctions` | The Treasury holds at least what fines brought in |
| `SettledIsEmpty`, `NoStrandedEscrow` | A closed task's escrow is empty; when the chain is done, no credit is stuck anywhere |
| `OnlyGovernanceMints` | Only the root budget is ever minted |

Deadlock checking is on: every run must end with the root task closed. **80,550 distinct states, no violation.**

### Witnesses

An invariant that can never be tested proves nothing, so `check.sh` also checks that the situations the invariants talk about are reachable. Each witness is checked as "this never happens", which TLC must violate:

| Witness | Shows |
|---|---|
| `Reach_Payment`, `Reach_PaymentOnBothHops` | Margins are paid, on both subcontract hops |
| `Reach_CapBinds` | The 20% cap actually limits a payment (credit left in escrow after paying) |
| `Reach_Clawback`, `Reach_Fine` | Clawbacks and fines happen |
| `Reach_NegativeEarnings` | Earnings can dip into the credit line |

## TaskLifecycle.tla: tasks, dependencies, rework, re-plans (0123)

See [design/task-lifecycle.md](../design/task-lifecycle.md) § What the model checks. **469,502 distinct states**, deadlock checking on.

| Witness | Shows |
|---|---|
| `Reach_AllAccepted` | The whole plan can be accepted |
| `Reach_ReplacementAccepted` | A replacement from a change set is accepted and its dependent too |
| `Reach_ReworkThenAccepted` | Rework, then acceptance on the second attempt |
| `Reach_SupersededWhileRunning` | A change set supersedes a task that had started |
| `Reach_Cascade` | A dependent of dead work is cancelled |

## RestoreMerge.tla: restoring the orchestrator and merging newer work (0135)

Crash and restore from any backup point, a forging VM, losing the machine as well, and owner review. **14,914 distinct states.**

| Invariant | Meaning |
|---|---|
| `AllSigned` | No unsigned (forged) entry is ever accepted into the orchestrator's log |
| `NoDoubleEffect` | No intended action takes effect twice across crash, restore and merge |
| `EffectsKnown` | Whenever the system runs, the orchestrator knows every effect that happened |
| `CancelledNeverRun` | A call cancelled because the harness never ran it never runs later |
| `NoForgedEffect` | A forged decision never causes an effect |

Witnesses: a tail is merged; a forged entry is dropped while the signed part merges; an owner review happens; a not-run call is cancelled; open calls are fenced after machine loss.

## Mutants (all caught)

| Mutant | Caught by |
|---|---|
| `AgentSession_NoTokenCheck`: effects allowed without the session-token check | `NoStaleEffect` |
| `AgentSession_NoFencingOnFinish`: results accepted from stale sessions | `OnlyCurrentSessionWrites` |
| `AgentSession_MoveKeepsOldSession`: a move reuses the old session token | `AtMostOneAccepted` |
| `Approval_AcceptAnyAuthority`: decisions accepted from anyone | `DecidedByChain` |
| `Approval_ExpireApproves`: expiry approves | `DecidedByChain` |
| `Economy_NoHeadroom`: payment ignores the chain margin cap | `MarginCap` |
| `Economy_SubcontractIsCost`: a subcontract price counts as real cost (margin stacking) | `ExternalIsMeteredSpend` |
| `Economy_DoublePay`: no once-only guard on payment | `NoDoublePayment` |
| `Economy_PayOnFail`: failed work is paid | `FailedIsNeverPaid` |
| `Economy_FineFromEscrow`: fines taken from task escrow | `EarningsFromWorkOnly` |
| `Economy_CloseWithOpenChildren`: a task closes while subcontracts are open | `NoStrandedEscrow` |
| `Economy_ClawbackIgnoresCreditLine`: clawback ignores the credit line | `EarningsAboveCreditLine` |
| `Economy_UnmeteredSpend`: spend leaves escrow but isn't recorded | `Conserved` |
| `TaskLifecycle_ReadyIgnoresDeps`: ready without checking dependencies | `DepsAcceptedBeforeWork` |
| `TaskLifecycle_SelfVerify`: the worker verifies its own work | `VerifierNotWorker` |
| `TaskLifecycle_ReplanKeepsRun`: superseding leaves the run alive | `NoRunOnDeadTask` |
| `TaskLifecycle_NoRewire`: dependents keep waiting on a superseded task | `NoDependencyOnSuperseded` |
| `TaskLifecycle_PayTwice`: no once-only payment guard | `PaidAtMostOnce` |
| `TaskLifecycle_ReplanAccepted`: a re-plan supersedes accepted work | `DepsAcceptedBeforeWork` |
| `TaskLifecycle_GiveUpLeavesRun`: giving up leaves the run alive | `NoRunOnDeadTask` |
| `TaskLifecycle_UnboundedRework`: no attempt limit | `AttemptsBounded` |
| `TaskLifecycle_NoCascade`: dependents of dead work are never cancelled | deadlock |
| `RestoreMerge_AcceptUnsigned`: merge trusts the machine's copy | `AllSigned` |
| `RestoreMerge_NoMerge`: the newer tail is thrown away and redone | `EffectsKnown` |
| `RestoreMerge_NoPauseOnMachineLoss`: carry on with an unknown window | `EffectsKnown` |
| `RestoreMerge_RunCancelled`: cancelled calls may still run | `CancelledNeverRun` |
| `RestoreMerge_RunFenced`: a replacement VM runs fenced calls | `EffectsKnown` |
| `RestoreMerge_OwnerRecordsDecision`: the owner's record is a runnable decision | `NoDoubleEffect` |

## What the mutants taught us

The first version of `AgentSession` only had `NoDoubleEffect`. Two mutants survived it:

- Without the token check, a VM on a returning machine could still cause a tool's external effect **after** the orchestrator had closed that call. Each call still ran at most once, so nothing fired. `NoStaleEffect` was added.
- Results from a stale VM could enter the transcript before the new VM's Hello. The workspace changes behind such a result live on a VM that's about to be discarded. `OnlyCurrentSessionWrites` was added.

Both are guarantees the design promised (0044, 0102); now they're checked.

Modelling the economy corrected the design itself (0121):

- The design said a payment is "real cost + margin". But the real cost is already spent from the child's escrow, so paying it again would pay it twice. **A payment is the margin only.**
- The ledger had no entry for moving a price from a parent's escrow into a child's. **`subcontract` is now its own entry kind**, and it is not real cost, so margin can't stack across hops.
- A clawback must stop at the credit line, and the chain's margin must be counted net of clawbacks.

Modelling the restore merge also corrected its design (0135): calls must be keyed by ids that are unique across restores, not by transcript position; and an owner-confirmed effect must be recorded as a result only, or a replacement VM could run it again.

