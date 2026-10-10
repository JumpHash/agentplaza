------------------------------ MODULE Approval ------------------------------
(* Approval escalation (0094): a request moves up its chain on deadlines and expires to deny
   after the last step. C-004 requests (irreversible, external, money-moving) have the chain
   <<"owner">> only (0051). Decisions can arrive late, from any authority, at any time. *)
EXTENDS Naturals, Sequences

CONSTANTS Authorities

\* Example chains: a town request, a council-level request, and a C-004 request (owner only).
Chains == {<<"mayor", "council", "owner">>, <<"council", "owner">>, <<"owner">>}

VARIABLES chain, pos, status, decidedBy, decidedAt

vars == <<chain, pos, status, decidedBy, decidedAt>>

Init ==
  /\ chain \in Chains
  /\ pos = 1
  /\ status = "pending"
  /\ decidedBy = "none"
  /\ decidedAt = 0

\* An authority answers. The orchestrator accepts it only from the authority at the current step.
Decide(a, approve) ==
  /\ status = "pending"
  /\ IF a = chain[pos]
       THEN /\ status' = IF approve THEN "approved" ELSE "denied"
            /\ decidedBy' = a
            /\ decidedAt' = pos
       ELSE UNCHANGED <<status, decidedBy, decidedAt>>     \* late or out-of-turn answer: rejected
  /\ UNCHANGED <<chain, pos>>

\* The current step's deadline passes.
Deadline ==
  /\ status = "pending"
  /\ IF pos < Len(chain)
       THEN /\ pos' = pos + 1 /\ UNCHANGED <<status, decidedBy, decidedAt>>
       ELSE /\ status' = "expired" /\ UNCHANGED <<pos, decidedBy, decidedAt>>
  /\ UNCHANGED chain

Next == \/ \E a \in Authorities, b \in BOOLEAN : Decide(a, b)
        \/ Deadline

Spec == Init /\ [][Next]_vars

IsC004 == chain = <<"owner">>

\* Only the authority at the step where it was decided can have decided it.
DecidedByChain == status \in {"approved", "denied"} => decidedBy = chain[decidedAt]

\* A C-004 request is only ever approved by the owner.
C004OnlyOwner == (IsC004 /\ status = "approved") => decidedBy = "owner"

\* Expiry means denial: nothing is approved by default.
ExpiredIsNotApproved == status = "expired" => decidedBy = "none"

\* The position never runs past the chain.
PosInChain == pos \in 1..Len(chain)

\* Once decided, a request stays decided (checked as an action property).
FinalStays == [][(status \in {"approved", "denied", "expired"}) => (status' = status)]_vars
=============================================================================
