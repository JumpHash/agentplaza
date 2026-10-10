--------------------------- MODULE TaskLifecycle ---------------------------
(* The task lifecycle (0123, design/task-lifecycle.md) with dependencies, rework, re-plans
   and cancellation. A re-plan is a change set (0081): it supersedes one task that isn't
   accepted, killing its run, adds a replacement with the same dependencies, and points
   the superseded task's dependents at the replacement. A failed task can be replaced the
   same way. A task whose dependency failed or was cancelled can be cancelled (cascade).
   Bidding (ready -> bidding -> assigned) is folded into Assign. *)
EXTENDS Naturals, FiniteSets

CONSTANTS Agents, MaxAttempts, MaxChangeSets

Ids == 1..4                        \* 1..3 are the planned tasks; 4 is free for a replacement
None == "none"
Live == {"draft", "ready", "assigned", "running", "blocked", "rework"}   \* can still be superseded or cancelled

VARIABLES st, outcome, deps, worker, verifier, attempts, live, paid, released, cs

vars == <<st, outcome, deps, worker, verifier, attempts, live, paid, released, cs>>

Out(t) == IF st[t] = "closed" THEN outcome[t] ELSE st[t]

Init ==
  /\ st = [t \in Ids |-> IF t <= 3 THEN "draft" ELSE "unused"]
  /\ outcome = [t \in Ids |-> None]
  /\ deps = [t \in Ids |-> CASE t = 2 -> {1} [] t = 3 -> {2} [] OTHER -> {}]   \* 1 <- 2 <- 3
  /\ worker = [t \in Ids |-> None]
  /\ verifier = [t \in Ids |-> None]
  /\ attempts = [t \in Ids |-> 0]
  /\ live = [t \in Ids |-> FALSE]
  /\ paid = [t \in Ids |-> 0]
  /\ released = [t \in Ids |-> 0]
  /\ cs = 0

Set(t, s) == st' = [st EXCEPT ![t] = s]

Ready(t) ==
  /\ st[t] = "draft" /\ \A d \in deps[t] : Out(d) = "accepted"
  /\ Set(t, "ready")
  /\ UNCHANGED <<outcome, deps, worker, verifier, attempts, live, paid, released, cs>>

Assign(t, w) ==
  /\ st[t] = "ready" /\ Set(t, "assigned") /\ worker' = [worker EXCEPT ![t] = w]
  /\ UNCHANGED <<outcome, deps, verifier, attempts, live, paid, released, cs>>

Start(t) ==
  /\ st[t] \in {"assigned", "rework"} /\ attempts[t] < MaxAttempts
  /\ Set(t, "running") /\ live' = [live EXCEPT ![t] = TRUE]
  /\ attempts' = [attempts EXCEPT ![t] = @ + 1]
  /\ UNCHANGED <<outcome, deps, worker, verifier, paid, released, cs>>

Block(t) ==
  /\ st[t] = "running" /\ Set(t, "blocked")
  /\ UNCHANGED <<outcome, deps, worker, verifier, attempts, live, paid, released, cs>>
Unblock(t) ==
  /\ st[t] = "blocked" /\ Set(t, "running")
  /\ UNCHANGED <<outcome, deps, worker, verifier, attempts, live, paid, released, cs>>

\* The worker submits a claim; its run ends; someone else verifies in a fresh VM (0053).
Claim(t, v) ==
  /\ st[t] = "running" /\ v # worker[t]
  /\ Set(t, "verifying") /\ verifier' = [verifier EXCEPT ![t] = v] /\ live' = [live EXCEPT ![t] = FALSE]
  /\ UNCHANGED <<outcome, deps, worker, attempts, paid, released, cs>>

Pass(t) ==
  /\ st[t] = "verifying" /\ Set(t, "accepted")
  /\ UNCHANGED <<outcome, deps, worker, verifier, attempts, live, paid, released, cs>>

Reject(t) ==
  /\ st[t] = "verifying"
  /\ Set(t, IF attempts[t] < MaxAttempts THEN "rework" ELSE "failed")
  /\ UNCHANGED <<outcome, deps, worker, verifier, attempts, live, paid, released, cs>>

\* Stop conditions (0098): budget, repeated failure, or the agent gives up.
GiveUp(t) ==
  /\ st[t] \in {"running", "blocked"} /\ Set(t, "failed") /\ live' = [live EXCEPT ![t] = FALSE]
  /\ UNCHANGED <<outcome, deps, worker, verifier, attempts, paid, released, cs>>

Pay(t) ==
  /\ st[t] = "accepted" /\ paid[t] = 0 /\ paid' = [paid EXCEPT ![t] = 1]
  /\ UNCHANGED <<st, outcome, deps, worker, verifier, attempts, live, released, cs>>

\* Close: escrow is released (refund) exactly once, and only with no run alive.
Close(t) ==
  /\ \/ st[t] = "accepted" /\ paid[t] = 1
     \/ st[t] \in {"failed", "cancelled", "superseded"}
  /\ ~live[t]
  /\ Set(t, "closed") /\ outcome' = [outcome EXCEPT ![t] = st[t]]
  /\ released' = [released EXCEPT ![t] = @ + 1]
  /\ UNCHANGED <<deps, worker, verifier, attempts, live, paid, cs>>

\* A change set replaces task t with a fresh task n.
Replan(t, n) ==
  /\ cs < MaxChangeSets /\ st[n] = "unused"
  /\ \/ st[t] \in Live
     \/ Out(t) = "failed"
  /\ st' = [st EXCEPT ![n] = "draft", ![t] = IF st[t] \in Live THEN "superseded" ELSE @]
  /\ live' = [live EXCEPT ![t] = FALSE]                       \* superseding kills its run
  /\ deps' = [u \in Ids |-> IF u = n THEN deps[t]
                            ELSE IF t \in deps[u] THEN (deps[u] \ {t}) \cup {n} ELSE deps[u]]
  /\ cs' = cs + 1
  /\ UNCHANGED <<outcome, worker, verifier, attempts, paid, released>>

\* Cascade: a task whose dependency is dead can be cancelled.
Cancel(t) ==
  /\ st[t] \in Live /\ \E d \in deps[t] : Out(d) \in {"failed", "cancelled"}
  /\ Set(t, "cancelled") /\ live' = [live EXCEPT ![t] = FALSE]
  /\ UNCHANGED <<outcome, deps, worker, verifier, attempts, paid, released, cs>>

AllClosed == \A t \in Ids : st[t] \in {"closed", "unused"}

Next ==
  \/ \E t \in Ids : Ready(t) \/ Start(t) \/ Block(t) \/ Unblock(t) \/ Pass(t) \/ Reject(t)
                    \/ GiveUp(t) \/ Pay(t) \/ Close(t) \/ Cancel(t)
  \/ \E t \in Ids, a \in Agents : Assign(t, a) \/ Claim(t, a)
  \/ \E t, n \in Ids : Replan(t, n)
  \/ AllClosed /\ UNCHANGED vars                              \* the only allowed end

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
DepsAcceptedBeforeWork ==
  \A t \in Ids : st[t] \in {"assigned", "running", "blocked", "verifying", "rework", "accepted"}
                 => \A d \in deps[t] : Out(d) = "accepted"
VerifierNotWorker == \A t \in Ids : verifier[t] # None => verifier[t] # worker[t]
NoRunOnDeadTask == \A t \in Ids : live[t] => st[t] \in {"running", "blocked"}
PaidOnlyIfAccepted == \A t \in Ids : paid[t] > 0 => Out(t) = "accepted"
PaidAtMostOnce == \A t \in Ids : paid[t] <= 1
ReleasedExactlyOnce == \A t \in Ids : released[t] = IF st[t] = "closed" THEN 1 ELSE 0
AcceptedIsPaidAtClose == \A t \in Ids : st[t] = "closed" /\ outcome[t] = "accepted" => paid[t] = 1
AttemptsBounded == \A t \in Ids : attempts[t] <= MaxAttempts
\* Nobody waits on a task a change set replaced.
NoDependencyOnSuperseded ==
  \A u \in Ids : st[u] \in Live => \A d \in deps[u] : Out(d) # "superseded"
NoSelfDependency == \A t \in Ids : t \notin deps[t]

\* Accepted work is never undone by a re-plan or anything else (an action property).
AcceptedStays == [][\A t \in Ids : Out(t) = "accepted" => Out(t)' = "accepted"]_vars
=============================================================================
