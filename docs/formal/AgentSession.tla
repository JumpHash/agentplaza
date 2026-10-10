---------------------------- MODULE AgentSession ----------------------------
(* One agent's life across VMs and machines (0029, 0084, 0085, 0091, 0102).
   Models: tool calls authorised and recorded by the orchestrator, snapshots at tool
   boundaries, background upload, machine failure and return, the owner moving the agent
   (MoveRun), restore on the same machine after a kill, the Hello/resume handshake, and
   fencing of stale VMs.
   The orchestrator accepts a request only from the current session (token check). *)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS Machines, MaxCalls, MaxVms

VARIABLES
  transcript,   \* Seq of [kind |-> "decision"|"result", call |-> Nat, reexec |-> BOOLEAN]
  current,      \* id of the session the orchestrator accepts
  nextSession,
  vms,          \* [1..MaxVms -> [m, session, applied, state, executing, ran]]
  snapshots,    \* set of [m, applied, uploaded]
  up,           \* [Machines -> BOOLEAN]
  nextCall,
  executed,     \* [1..MaxCalls -> Nat]: external side effects per call (egress goes through the orchestrator)
  staleEffects, \* external effects caused by a non-current session or for an already-closed call
  staleWrites   \* transcript entries written by a session that wasn't current at the time

vars == <<transcript, current, nextSession, vms, snapshots, up, nextCall, executed, staleEffects, staleWrites>>
Calls == 1..MaxCalls
VmIds == 1..MaxVms
States == {"unused", "running", "restoring", "dead", "discarded"}

Valid(v) == vms[v].session = current
Live(v)  == vms[v].state \in {"running", "restoring"}
CanAct(v) == vms[v].state = "running" /\ up[vms[v].m]

HasResult(c) == \E i \in 1..Len(transcript) : transcript[i].kind = "result" /\ transcript[i].call = c
HasDecision(c) == \E i \in 1..Len(transcript) : transcript[i].kind = "decision" /\ transcript[i].call = c
Unfinished == {c \in Calls : HasDecision(c) /\ ~HasResult(c)}

Init ==
  /\ transcript = <<>>
  /\ current = 1
  /\ nextSession = 2
  /\ vms = [v \in VmIds |-> IF v = 1
              THEN [m |-> CHOOSE m \in Machines : TRUE, session |-> 1, applied |-> 0, state |-> "running", executing |-> 0, ran |-> FALSE]
              ELSE [m |-> CHOOSE m \in Machines : TRUE, session |-> 0, applied |-> 0, state |-> "unused", executing |-> 0, ran |-> FALSE]]
  /\ snapshots = {}
  /\ up = [m \in Machines |-> TRUE]
  /\ nextCall = 1
  /\ executed = [c \in Calls |-> 0]
  /\ staleEffects = 0
  /\ staleWrites = 0

\* Harness asks to run a tool; orchestrator checks the token, records the decision (ToolRequest -> ToolDecision).
Request(v) ==
  /\ CanAct(v) /\ Valid(v)
  /\ vms[v].executing = 0
  /\ vms[v].applied = Len(transcript)          \* harness is caught up
  /\ nextCall <= MaxCalls
  /\ transcript' = Append(transcript, [kind |-> "decision", call |-> nextCall, reexec |-> FALSE])
  /\ vms' = [vms EXCEPT ![v].executing = nextCall, ![v].applied = Len(transcript) + 1, ![v].ran = FALSE]
  /\ nextCall' = nextCall + 1
  /\ UNCHANGED <<current, nextSession, snapshots, up, executed, staleEffects, staleWrites>>

\* The tool's external effect (e.g. an egress call): authorised per request by the orchestrator, so only the current session can cause it.
Execute(v) ==
  /\ CanAct(v) /\ Valid(v)
  /\ vms[v].executing # 0 /\ ~vms[v].ran
  /\ executed' = [executed EXCEPT ![vms[v].executing] = @ + 1]
  /\ staleEffects' = IF ~Valid(v) \/ HasResult(vms[v].executing) THEN staleEffects + 1 ELSE staleEffects
  /\ vms' = [vms EXCEPT ![v].ran = TRUE]
  /\ UNCHANGED <<transcript, current, nextSession, snapshots, up, nextCall, staleWrites>>

\* Harness reports the result; accepted only from the current session.
Finish(v) ==
  /\ CanAct(v)
  /\ vms[v].executing # 0 /\ vms[v].ran
  /\ IF Valid(v) /\ ~HasResult(vms[v].executing)
       THEN /\ transcript' = Append(transcript, [kind |-> "result", call |-> vms[v].executing, reexec |-> FALSE])
            /\ staleWrites' = IF Valid(v) THEN staleWrites ELSE staleWrites + 1
            /\ vms' = [vms EXCEPT ![v].executing = 0, ![v].applied = Len(transcript) + 1]
       ELSE /\ transcript' = transcript                     \* stale session: rejected
            /\ vms' = [vms EXCEPT ![v].executing = 0]
            /\ staleWrites' = staleWrites
  /\ UNCHANGED <<current, nextSession, snapshots, up, nextCall, executed, staleEffects>>

\* Snapshot at a tool boundary, written to the machine's local cache (0043, 0091).
Snapshot(v) ==
  /\ CanAct(v) /\ vms[v].executing = 0
  /\ snapshots' = snapshots \cup {[m |-> vms[v].m, applied |-> vms[v].applied, uploaded |-> FALSE]}
  /\ UNCHANGED <<transcript, current, nextSession, vms, up, nextCall, executed, staleEffects, staleWrites>>

Upload(s) ==
  /\ s \in snapshots /\ ~s.uploaded /\ up[s.m]
  /\ snapshots' = (snapshots \ {s}) \cup {[s EXCEPT !.uploaded = TRUE]}
  /\ UNCHANGED <<transcript, current, nextSession, vms, up, nextCall, executed, staleEffects, staleWrites>>

MachineFail(m) ==
  /\ up[m]
  /\ up' = [up EXCEPT ![m] = FALSE]
  /\ UNCHANGED <<transcript, current, nextSession, vms, snapshots, nextCall, executed, staleEffects, staleWrites>>

MachineReturn(m) ==
  /\ ~up[m]
  /\ up' = [up EXCEPT ![m] = TRUE]
  /\ UNCHANGED <<transcript, current, nextSession, vms, snapshots, nextCall, executed, staleEffects, staleWrites>>

\* The VMM process dies while the machine stays up (kill -9, 0027).
Kill(v) ==
  /\ CanAct(v)
  /\ vms' = [vms EXCEPT ![v].state = "dead"]
  /\ UNCHANGED <<transcript, current, nextSession, snapshots, up, nextCall, executed, staleEffects, staleWrites>>

\* Start a VM from a snapshot (or from scratch) with a fresh session; the orchestrator now accepts only it.
StartFrom(m, applied) ==
  \E w \in VmIds :
    /\ vms[w].state = "unused"
    /\ vms' = [vms EXCEPT ![w] = [m |-> m, session |-> nextSession, applied |-> applied, state |-> "restoring", executing |-> 0, ran |-> FALSE]]
    /\ current' = nextSession
    /\ nextSession' = nextSession + 1

\* Resume on the same machine from a local snapshot after a kill.
RestoreLocal(v) ==
  /\ vms[v].state = "dead" /\ Valid(v) /\ up[vms[v].m]
  /\ \E s \in snapshots : s.m = vms[v].m /\ StartFrom(s.m, s.applied)
  /\ UNCHANGED <<transcript, snapshots, up, nextCall, executed, staleEffects, staleWrites>>

\* Owner moves the agent (MoveRun, 0102): from the last uploaded snapshot, or from the transcript alone.
Move ==
  /\ \E v \in VmIds : Live(v) /\ Valid(v) /\ ~up[vms[v].m]   \* its machine is unreachable
  /\ \E m \in Machines :
       /\ up[m]
       /\ \/ \E s \in snapshots : s.uploaded /\ StartFrom(m, s.applied)
          \/ StartFrom(m, 0)
  /\ UNCHANGED <<transcript, snapshots, up, nextCall, executed, staleEffects, staleWrites>>

\* Hello after (re)start: the orchestrator closes unfinished calls as "possibly re-executed" and
\* returns everything after the harness's last seq (0084).
Hello(v) ==
  /\ vms[v].state = "restoring" /\ up[vms[v].m] /\ Valid(v)
  /\ transcript' = IF Unfinished = {} THEN transcript
                   ELSE Append(transcript, [kind |-> "result", call |-> CHOOSE c \in Unfinished : TRUE, reexec |-> TRUE])
  /\ vms' = [vms EXCEPT ![v].state = IF Cardinality(Unfinished) <= 1 THEN "running" ELSE "restoring",
                          ![v].applied = Len(transcript')]
  /\ UNCHANGED <<current, nextSession, snapshots, up, nextCall, executed, staleEffects, staleWrites>>

\* Fencing (0102): a returning machine is told to discard VMs whose session is no longer current.
Fence(v) ==
  /\ Live(v) /\ ~Valid(v) /\ up[vms[v].m]
  /\ vms' = [vms EXCEPT ![v].state = "discarded"]
  /\ UNCHANGED <<transcript, current, nextSession, snapshots, up, nextCall, executed, staleEffects, staleWrites>>

Next ==
  \/ \E v \in VmIds : Request(v) \/ Execute(v) \/ Finish(v) \/ Snapshot(v) \/ Kill(v) \/ RestoreLocal(v) \/ Hello(v) \/ Fence(v)
  \/ \E s \in snapshots : Upload(s)
  \/ \E m \in Machines : MachineFail(m) \/ MachineReturn(m)
  \/ Move

Spec == Init /\ [][Next]_vars

\* ---------------- Invariants ----------------
TypeOK ==
  /\ current \in Nat /\ nextCall \in 1..(MaxCalls + 1)
  /\ \A v \in VmIds : vms[v].state \in States

\* Never two copies of the agent that the orchestrator would accept.
AtMostOneAccepted == Cardinality({v \in VmIds : Live(v) /\ Valid(v)}) <= 1

\* No external side effect of a tool call happens twice.
NoDoubleEffect == \A c \in Calls : executed[c] <= 1

\* Every call has at most one decision and at most one result, and results follow decisions.
TranscriptWellFormed ==
  \A c \in Calls :
    /\ Cardinality({i \in 1..Len(transcript) : transcript[i].kind = "decision" /\ transcript[i].call = c}) <= 1
    /\ Cardinality({i \in 1..Len(transcript) : transcript[i].kind = "result" /\ transcript[i].call = c}) <= 1
    /\ \A i \in 1..Len(transcript) : transcript[i].kind = "result" /\ transcript[i].call = c =>
          \E j \in 1..(i-1) : transcript[j].kind = "decision" /\ transcript[j].call = c

\* No external effect comes from a stale session, or happens for a call the orchestrator already closed.
NoStaleEffect == staleEffects = 0

\* Fencing: the transcript is only ever written by the session that is current at that moment.
OnlyCurrentSessionWrites == staleWrites = 0

\* A harness never believes in transcript entries the orchestrator doesn't have.
HarnessIsPrefix == \A v \in VmIds : vms[v].applied <= Len(transcript)

=============================================================================
