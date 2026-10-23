# Formal models (jhrl machines)

The formal models are four jhrl machines in `crates/jhrl/specs/machines/`: AgentSession, Approval, TaskLifecycle and RestoreMerge. Each is model-checked by jhrl's own explicit-state checker. Each mutant (a deliberately broken copy, written in the file as `mutant … { replace … expect violation … }`) must be caught by the invariant it names. Each witness (a state the model must be able to reach) must be reachable, so an invariant can't pass vacuously.

## Running the checks

```sh
cargo run --release -p jhrl -- check crates/jhrl/specs/machines/*.jhrl
```

This checks every model, mutant and witness.

```sh
cargo test -p jhrl --test specs
```

This runs the parse and name checks and the state-count pins. The mutant and witness sweeps are `#[ignore]`d, so run them with:

```sh
cargo test --release -p jhrl --test specs -- --ignored
```

## AgentSession: one agent across VMs and machines

Covers tool calls authorised and recorded by the orchestrator (0084), snapshots at tool boundaries and background upload (0043, 0091), machine failure and return, the owner's move of the agent (`Move`, 0102), restore after a kill (0027), the Hello/resume handshake (0084) and fencing (0102).

State counts: the `quick` model (2 machines, 2 tool calls, 2 VMs) has 64,316 distinct states. The `full bounds` model (2 machines, 2 tool calls, 3 VMs) has 4,144,576 distinct states; checking it takes about 17 minutes and 22 GB of memory on a 24-core machine, so `jhrl check` on this file is slow, and the mutants and witnesses use `quick`.

| Invariant | Meaning |
|---|---|
| At most one copy of the agent is accepted | Never two copies of the agent that the orchestrator would accept |
| No double external effect | No tool call's external effect happens twice |
| No external effect from a stale session | No external effect comes from a stale session, or for a call already closed |
| Only the current session writes the transcript | The transcript is only written by the session current at that moment |
| Transcript is well formed | ≤ 1 decision and ≤ 1 result per call; results follow decisions |
| The harness is never ahead of the transcript | A harness never believes in entries the orchestrator doesn't have |
| Call numbers stay in range | The next call number stays between 1 and MaxCalls + 1 |

### AgentSession witnesses

| Witness | Shows |
|---|---|
| A moved agent runs on another machine | After the old machine goes down, the moved agent runs on a different one |
| A resume closes an unfinished call as possibly re-executed | The Hello handshake closes an unfinished call, marked as possibly re-executed |

## Approval: escalation chains (0094)

A request moves up its chain on deadlines and expires to deny after the last step (AP-038). C-004 requests (irreversible, external, money-moving) have the chain `["owner"]` only (AP-052). Decisions can arrive late, from any authority, at any time. The model has two chains (a town request, orchestrator then owner, and a C-004 request, owner only) and three authorities.

State count for the `two chains, three authorities` model: 11 distinct states.

| Invariant | Meaning |
|---|---|
| Decided by the chain | Only the authority at the current step can decide; late or out-of-turn answers are rejected |
| C-004 is approved only by the owner | C-004 requests are only ever approved by the owner |
| Expiry is not approval | Silence never means yes |
| Position stays in the chain | The current step is always a position in the chain |
| A decision is final (transition) | Once decided or expired, a request never changes |

### Approval witnesses

| Witness | Shows |
|---|---|
| A town request escalates to the owner | The town request reaches the owner's step while still pending |
| An escalated request is approved by the owner | The owner approves a request that escalated from the orchestrator |

## TaskLifecycle: tasks, dependencies, rework, re-plans (0123)

Tasks with dependencies, rework, re-plans and cancellation (AP-029). The model has three planned tasks, 1 ← 2 ← 3 (each depends on the one before), and one free id, 4, for a replacement. A re-plan is a change set (AP-061): it supersedes a task that has not been accepted, kills its run, adds a replacement with the same dependencies, and points the superseded task's dependents at the replacement. A task whose dependency failed or was cancelled can be cancelled (cascade). Bidding is folded into `Assign`.

State count for the `three tasks, two agents` model (2 agents, up to 2 attempts, 1 change set): 469,502 distinct states (the figure recorded with the earlier model; not yet reproduced by jhrl). Deadlock checking is on.

| Invariant | Meaning |
|---|---|
| Dependencies are accepted before work | A task is assigned, running, blocked, verifying, rework or accepted only after its dependencies are accepted |
| The verifier is not the worker | A task is never verified by the agent that worked on it |
| No run on a dead task | A live run belongs only to a running or blocked task |
| Paid only if accepted | Payment happens only for accepted work |
| Paid at most once | No task is paid more than once |
| Accepted work is paid at close | A task closed as accepted has been paid |
| Attempts are bounded | Rework attempts never exceed `MaxAttempts` |
| Nobody waits on superseded work | No open task waits on a superseded task |
| No task depends on itself | No task depends on itself |
| Accepted work stays accepted (transition) | Accepted work is never undone |

### TaskLifecycle witnesses

| Witness | Shows |
|---|---|
| The whole plan is accepted | Tasks 1 to 3 all close as accepted |
| A replacement and its dependent are accepted | A replacement (task 4) and its dependent (task 3) are accepted |
| Rework, then accepted on the second attempt | Task 1 is reworked and accepted on its second attempt |
| A change set supersedes a task that had started | A change set supersedes task 2 after it had an attempt |
| A dependent of dead work is cancelled | Task 3 is cancelled when its dependency dies |

## RestoreMerge: restoring the orchestrator and merging newer work (0135)

Crash and restore from any backup point, a forging VM, losing the machine as well, and owner review.

State count for the `two intents, log up to seven` model: 14,914 distinct states. jhrl reproduces this count exactly.

| Invariant | Meaning |
|---|---|
| Every log entry is signed | No unsigned (forged) entry is ever accepted into the orchestrator's log |
| No effect runs twice | No intended action takes effect twice across crash, restore and merge |
| Effects are known while running | Whenever the system runs, the orchestrator knows every effect that happened |
| A cancelled call never runs | A call cancelled because the harness never ran it never runs later |
| A forged decision never has an effect | A forged decision never causes an effect |

### RestoreMerge witnesses

| Witness | Shows |
|---|---|
| A merge recovers a tail newer than the backup | Merging the journal recovers entries newer than the backup |
| A forged entry is dropped on merge | A forged entry is dropped while the signed tail is merged |
| The owner reviews an intent | The owner's review of the unknown window happens |
| A call is cancelled because the harness never ran it | An open call the harness never ran is cancelled |
| A lost machine fences an open call | After machine loss, an open call is fenced |

## Mutants

Twenty mutants, each written in its machine's file. The `expect` line names the invariant that must catch it (or, for one mutant, deadlock checking).

| Machine | Mutant | Caught by |
|---|---|---|
| AgentSession | No session token check on effects | No external effect from a stale session |
| AgentSession | Results accepted from stale sessions | Only the current session writes the transcript |
| AgentSession | A move reuses the old session token | At most one copy of the agent is accepted |
| Approval | Accept any authority | Decided by the chain |
| Approval | Silence means yes | Decided by the chain |
| TaskLifecycle | Ready without checking dependencies | Dependencies are accepted before work |
| TaskLifecycle | The worker verifies its own work | The verifier is not the worker |
| TaskLifecycle | Superseding leaves the run alive | No run on a dead task |
| TaskLifecycle | Dependents keep waiting on the superseded task | Nobody waits on superseded work |
| TaskLifecycle | No once-only payment guard | Paid at most once |
| TaskLifecycle | A re-plan supersedes accepted work | Dependencies are accepted before work |
| TaskLifecycle | Giving up leaves the run alive | No run on a dead task |
| TaskLifecycle | No attempt limit | Attempts are bounded |
| TaskLifecycle | Dependents of dead work are never cancelled | Deadlock (`expect deadlock`) |
| RestoreMerge | Accept an unsigned journal | Every log entry is signed |
| RestoreMerge | Throw away the newer tail | Effects are known while running |
| RestoreMerge | Carry on while the window is unknown | Effects are known while running |
| RestoreMerge | Run cancelled calls | A cancelled call never runs |
| RestoreMerge | Run fenced calls | Effects are known while running |
| RestoreMerge | Owner record is a runnable decision | No effect runs twice |

## What the mutants taught us

The first version of AgentSession only checked `No double external effect`. Two mutants survived it:

- Without the token check, a VM on a returning machine could still cause a tool's external effect **after** the orchestrator had closed that call. Each call still ran at most once, so nothing fired. `No external effect from a stale session` was added.
- Results from a stale VM could enter the transcript before the new VM's Hello. The workspace changes behind such a result live on a VM that's about to be discarded. `Only the current session writes the transcript` was added.

Both are guarantees the design promised (0044, 0102); now they're checked.

Modelling the restore merge also corrected its design (0135): calls must be keyed by ids that are unique across restores, not by transcript position; and an owner-confirmed effect must be recorded as a result only, or a replacement VM could run it again.
