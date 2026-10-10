---------------------------- MODULE RestoreMerge ----------------------------
(* Restoring the orchestrator from a backup and merging newer work back (0135).
   The orchestrator signs every transcript entry it commits (hash-chained). A tool call's
   external effect happens only after the decision reached the harness. The machine daemon
   journals every entry it relays, so a lost VM doesn't lose the tail; a lost machine does.
   After a restore the orchestrator merges the longest *signed* extension of its log from the
   machine's journal. Open calls the harness never ran are cancelled; if the machine is lost,
   the run pauses until the owner reviews the unknown window against provider logs.
   A compromised VM may append forged (unsigned) entries to its copy. *)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS Intents, MaxLen

VARIABLES db, j, backup, crashed, merged, machineLost, paused, reviewed, executed, ranAt, cancelled, fenced, forged,
          nextId,    \* call ids are unique across restores (ULIDs in the real system)
          tailMerged \* ghost: a merge recovered entries newer than the backup

vars == <<db, j, backup, crashed, merged, machineLost, paused, reviewed, executed, ranAt, cancelled, fenced, forged, nextId, tailMerged>>

Ent(k, i, c) == [k |-> k, i |-> i, c |-> c, sig |-> TRUE]   \* k: "d" decision, "r" result, "x" void; c: call id
Pos(s, i) == {p \in 1..Len(s) : s[p].i = i}
LastPos(s, i) == IF Pos(s, i) = {} THEN 0 ELSE CHOOSE p \in Pos(s, i) : \A q \in Pos(s, i) : q <= p
StatusIn(s, i) == IF Pos(s, i) = {} THEN "none"
                  ELSE CASE s[LastPos(s, i)].k = "d" -> "open" [] s[LastPos(s, i)].k = "r" -> "done" [] OTHER -> "void"
Status(i) == StatusIn(db, i)
IsPrefix(a, b) == Len(a) <= Len(b) /\ a = SubSeq(b, 1, Len(a))
SignedLen(s) == IF \A p \in 1..Len(s) : s[p].sig THEN Len(s) ELSE (CHOOSE p \in 1..Len(s) : ~s[p].sig /\ \A q \in 1..p-1 : s[q].sig) - 1
SignedPrefix(s) == SubSeq(s, 1, SignedLen(s))
Operating == (~crashed \/ merged) /\ ~paused

Init ==
  /\ db = <<>> /\ j = <<>> /\ backup = <<>>
  /\ crashed = FALSE /\ merged = FALSE /\ machineLost = FALSE /\ paused = FALSE /\ reviewed = {}
  /\ executed = [i \in Intents |-> 0] /\ ranAt = {} /\ cancelled = {} /\ fenced = {} /\ forged = FALSE /\ nextId = 1 /\ tailMerged = FALSE

\* The agent asks for a tool call; the orchestrator signs and commits the decision.
Request(i) ==
  /\ Operating /\ Len(db) < MaxLen
  /\ Status(i) \in {"none", "void"}
  /\ db' = Append(db, Ent("d", i, nextId))
  /\ nextId' = nextId + 1
  /\ UNCHANGED <<j, backup, crashed, merged, machineLost, paused, reviewed, executed, ranAt, cancelled, fenced, forged, tailMerged>>

\* The machine relays the next committed entry to the harness and journals it.
Deliver ==
  /\ ~machineLost /\ Len(j) < Len(db) /\ IsPrefix(j, db)
  /\ j' = Append(j, db[Len(j) + 1])
  /\ UNCHANGED <<db, backup, crashed, merged, machineLost, paused, reviewed, executed, ranAt, cancelled, fenced, forged, nextId, tailMerged>>

\* The tool's external effect: only for a signed decision the harness has, once, and not if cancelled.
Effect(p) ==
  /\ ~machineLost /\ p \in 1..Len(j) /\ j[p].k = "d" /\ j[p].sig
  /\ j[p].c \notin ranAt /\ j[p].c \notin cancelled /\ j[p].c \notin fenced
  /\ executed' = [executed EXCEPT ![j[p].i] = @ + 1]
  /\ ranAt' = ranAt \cup {j[p].c}
  /\ UNCHANGED <<db, j, backup, crashed, merged, machineLost, paused, reviewed, cancelled, fenced, forged, nextId, tailMerged>>

\* The harness reports the result; the orchestrator accepts it only for the decision it has.
Result(p) ==
  /\ Operating /\ ~machineLost /\ p <= Len(db) /\ p <= Len(j) /\ db[p] = j[p] /\ j[p].c \in ranAt
  /\ Status(j[p].i) = "open" /\ LastPos(db, j[p].i) = p /\ Len(db) < MaxLen
  /\ db' = Append(db, Ent("r", j[p].i, j[p].c))
  /\ UNCHANGED <<j, backup, crashed, merged, machineLost, paused, reviewed, executed, ranAt, cancelled, fenced, forged, nextId, tailMerged>>

Backup == /\ ~crashed /\ backup' = db
          /\ UNCHANGED <<db, j, crashed, merged, machineLost, paused, reviewed, executed, ranAt, cancelled, fenced, forged, nextId, tailMerged>>

\* The orchestrator host dies and is restored from the last backup.
Crash == /\ ~crashed /\ db' = backup /\ crashed' = TRUE
         /\ UNCHANGED <<j, backup, merged, machineLost, paused, reviewed, executed, ranAt, cancelled, fenced, forged, nextId, tailMerged>>

\* A compromised VM appends a forged entry to what the machine relays.
Forge == /\ ~forged /\ ~machineLost /\ Len(j) < MaxLen
         /\ j' = Append(j, [k |-> "d", i |-> CHOOSE i \in Intents : TRUE, c |-> 99, sig |-> FALSE]) /\ forged' = TRUE
         /\ UNCHANGED <<db, backup, crashed, merged, machineLost, paused, reviewed, executed, ranAt, cancelled, fenced, nextId, tailMerged>>

\* While the orchestrator is down, the machine itself is lost (journal gone).
LoseMachine == /\ crashed /\ ~merged /\ ~machineLost /\ machineLost' = TRUE /\ j' = <<>>
               /\ UNCHANGED <<db, backup, crashed, merged, paused, reviewed, executed, ranAt, cancelled, fenced, forged, nextId, tailMerged>>

\* Merge: take the longest signed extension of the restored log from the machine's journal.
Merge ==
  /\ crashed /\ ~merged
  /\ LET sp  == SignedPrefix(j)
         ext == IF Len(sp) > Len(db) /\ IsPrefix(db, sp) THEN sp ELSE db
         openIds == {ext[LastPos(ext, i)].c : i \in {i2 \in Intents : StatusIn(ext, i2) = "open"}}
         notRun  == openIds \ ranAt
     IN /\ db' = ext
        /\ tailMerged' = (Len(ext) > Len(db))
        /\ j' = ext                                         \* the harness resyncs to the merged log
        /\ IF machineLost
             THEN /\ paused' = TRUE                         \* the window is unknown: owner review
                  /\ fenced' = fenced \cup openIds
                  /\ UNCHANGED cancelled
             ELSE /\ paused' = FALSE
                  /\ cancelled' = cancelled \cup notRun     \* the harness reports what it never ran
                  /\ UNCHANGED fenced
  /\ merged' = TRUE /\ machineLost' = FALSE                \* a replacement machine takes over
  /\ UNCHANGED <<backup, crashed, reviewed, executed, ranAt, forged, nextId>>

\* A cancelled open call is voided, so the agent may request it again.
Void(i) ==
  /\ Operating /\ Status(i) = "open" /\ db[LastPos(db, i)].c \in cancelled /\ Len(db) < MaxLen
  /\ db' = Append(db, Ent("x", i, db[LastPos(db, i)].c))
  /\ UNCHANGED <<j, backup, crashed, merged, machineLost, paused, reviewed, executed, ranAt, cancelled, fenced, forged, nextId, tailMerged>>

\* Owner review of the unknown window (machine lost): the owner checks provider and egress logs.
Review(i) ==
  /\ paused /\ i \notin reviewed /\ Len(db) < MaxLen
  /\ db' = CASE Status(i) = "open" /\ executed[i] > 0 -> Append(db, Ent("r", i, db[LastPos(db, i)].c))
           []   Status(i) = "open"                    -> Append(db, Ent("x", i, db[LastPos(db, i)].c))
           []   Status(i) \in {"none", "void"} /\ executed[i] > 0 -> Append(db, Ent("r", i, 0))   \* a record of what happened, never a runnable decision
           []   OTHER                                 -> db
  /\ reviewed' = reviewed \cup {i}
  /\ paused' = ((reviewed \cup {i}) # Intents)
  /\ UNCHANGED <<j, backup, crashed, merged, machineLost, executed, ranAt, cancelled, fenced, forged, nextId, tailMerged>>

Next ==
  \/ \E i \in Intents : Request(i) \/ Void(i) \/ Review(i)
  \/ \E p \in 1..MaxLen : Effect(p) \/ Result(p)
  \/ Deliver \/ Backup \/ Crash \/ Forge \/ LoseMachine \/ Merge

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
\* No forged entry ever enters the orchestrator's log.
AllSigned == \A p \in 1..Len(db) : db[p].sig
\* No intended action takes effect twice, across crash, restore and merge.
NoDoubleEffect == \A i \in Intents : executed[i] <= 1
\* Whenever the system runs, the orchestrator knows about every effect that happened.
EffectsKnown == Operating => \A i \in Intents : executed[i] > 0 => Status(i) \in {"open", "done"}
\* A call cancelled because the harness never ran it never runs later.
CancelledNeverRun == cancelled \cap ranAt = {}
\* A forged decision never causes an effect.
NoForgedEffect == 99 \notin ranAt
=============================================================================
