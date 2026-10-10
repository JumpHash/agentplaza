------------------------------ MODULE Economy ------------------------------
(* Task escrow, subcontracts, payments, refunds, clawbacks and fines in one task chain
   (0039, 0109-0112; ledger in treasury-draft/schema.sql).

   The owner mints a budget into the root task's escrow. A task's agent spends real cost
   from its escrow, or subcontracts: the price moves from the parent's escrow into the
   child's. A verified child is paid a margin from what is left in its escrow, capped so the
   chain's total margin stays within CapPct of the chain's real cost (0111); the rest is
   refunded to the parent. A failed child is refunded and paid nothing. Inside the warranty a
   paid margin can be clawed back to the Treasury, and fines come from earnings only; both
   stop at the agent's credit line. Every step is one balanced ledger entry. *)
EXTENDS Naturals, Integers, FiniteSets

CONSTANTS Agents, Budget, Step, Prices, CapPct, CreditLimit

Tasks == {"root", "s1", "s2"}
Subs  == {"s1", "s2"}
Parent(t) == IF t = "s1" THEN "root" ELSE "s1"           \* root -> s1 -> s2 (depth limit, 0039)
Children(t) == {c \in Subs : Parent(c) = t}

VARIABLES
  mint, tre, ext,          \* mint (goes negative), Treasury, external spend (real money out)
  esc, earn,               \* escrow per task, earnings per agent
  st,                      \* "none" | "open" | "passed" | "failed" | "settled"
  who,                     \* contractor of each subtask
  cost,                    \* real cost each task has spent
  margin,                  \* margin paid on each task, net of clawbacks
  pays,                    \* payment entries posted per task
  clawed, fines

vars == <<mint, tre, ext, esc, earn, st, who, cost, margin, pays, clawed, fines>>

Sum(f, S) == LET R[T \in SUBSET S] == IF T = {} THEN 0 ELSE LET x == CHOOSE x \in T : TRUE IN f[x] + R[T \ {x}]
             IN R[S]
Min(a, b) == IF a < b THEN a ELSE b
Max(a, b) == IF a > b THEN a ELSE b

ChainCost   == Sum(cost, Tasks)
ChainMargin == Sum(margin, Tasks)
\* What the Treasury may still pay as margin in this chain (checked before every payment, 0111).
Headroom == Max(0, (ChainCost * CapPct) \div 100 - ChainMargin)

Init ==
  /\ mint = 0 /\ tre = 0 /\ ext = 0
  /\ esc = [t \in Tasks |-> 0]
  /\ earn = [a \in Agents |-> 0]
  /\ st = [t \in Tasks |-> "none"]
  /\ who = [t \in Tasks |-> "owner"]
  /\ cost = [t \in Tasks |-> 0]
  /\ margin = [t \in Tasks |-> 0]
  /\ pays = [t \in Tasks |-> 0]
  /\ clawed = [t \in Tasks |-> FALSE]
  /\ fines = 0

\* Governance funds the root task (mint entry).
Fund ==
  /\ st["root"] = "none"
  /\ mint' = mint - Budget
  /\ esc' = [esc EXCEPT !["root"] = Budget]
  /\ st' = [st EXCEPT !["root"] = "open"]
  /\ UNCHANGED <<tre, ext, earn, who, cost, margin, pays, clawed, fines>>

\* Metered real cost: escrow -> external spend.
Spend(t) ==
  /\ st[t] = "open" /\ esc[t] >= Step
  /\ esc' = [esc EXCEPT ![t] = @ - Step]
  /\ ext' = ext + Step
  /\ cost' = [cost EXCEPT ![t] = @ + Step]
  /\ UNCHANGED <<mint, tre, earn, st, who, margin, pays, clawed, fines>>

\* Award: the parent's escrow locks the price in the child's escrow (subcontract entry).
Award(t, a, p) ==
  /\ st[t] = "none" /\ st[Parent(t)] = "open" /\ esc[Parent(t)] >= p
  /\ esc' = [esc EXCEPT ![Parent(t)] = @ - p, ![t] = p]
  /\ st' = [st EXCEPT ![t] = "open"]
  /\ who' = [who EXCEPT ![t] = a]
  /\ UNCHANGED <<mint, tre, ext, earn, cost, margin, pays, clawed, fines>>

\* Verification in a fresh VM (0053), once the task's own subcontracts are closed.
Verify(t, ok) ==
  /\ t \in Subs /\ st[t] = "open"
  /\ \A c \in Children(t) : st[c] \in {"none", "settled"}
  /\ st' = [st EXCEPT ![t] = IF ok THEN "passed" ELSE "failed"]
  /\ UNCHANGED <<mint, tre, ext, esc, earn, who, cost, margin, pays, clawed, fines>>

\* Payment on verified work: the margin, from what is left in the child's escrow.
Pay(t) ==
  /\ st[t] = "passed" /\ pays[t] = 0
  /\ LET m == Min(esc[t], Headroom) IN
       /\ m > 0
       /\ esc' = [esc EXCEPT ![t] = @ - m]
       /\ earn' = [earn EXCEPT ![who[t]] = @ + m]
       /\ margin' = [margin EXCEPT ![t] = @ + m]
       /\ pays' = [pays EXCEPT ![t] = @ + 1]
  /\ UNCHANGED <<mint, tre, ext, st, who, cost, clawed, fines>>

\* Close a task: refund what's left to the parent (or to the mint for the root).
Close(t) ==
  /\ \/ t \in Subs /\ st[t] \in {"passed", "failed"}
     \/ t = "root" /\ st[t] = "open"
  /\ \A c \in Children(t) : st[c] \in {"none", "settled"}
  /\ IF t = "root"
       THEN /\ mint' = mint + esc[t] /\ esc' = [esc EXCEPT ![t] = 0]
       ELSE /\ esc' = [esc EXCEPT ![t] = 0, ![Parent(t)] = @ + esc[t]] /\ UNCHANGED mint
  /\ st' = [st EXCEPT ![t] = "settled"]
  /\ UNCHANGED <<tre, ext, earn, who, cost, margin, pays, clawed, fines>>

\* Warranty claim: the margin goes back to the Treasury, as far as the credit line allows.
Clawback(t) ==
  /\ t \in Subs /\ st[t] = "settled" /\ margin[t] > 0 /\ ~clawed[t]
  /\ LET amt == Min(margin[t], earn[who[t]] + CreditLimit) IN
       /\ amt > 0
       /\ earn' = [earn EXCEPT ![who[t]] = @ - amt]
       /\ tre' = tre + amt
       /\ margin' = [margin EXCEPT ![t] = @ - amt]
  /\ clawed' = [clawed EXCEPT ![t] = TRUE]
  /\ UNCHANGED <<mint, ext, esc, st, who, cost, pays, fines>>

\* A sanction (S-1xx, sanctions.yaml): earnings -> Treasury, never escrow.
Fine(a) ==
  /\ earn[a] - 1 >= -CreditLimit /\ fines < 2
  /\ earn' = [earn EXCEPT ![a] = @ - 1]
  /\ tre' = tre + 1
  /\ fines' = fines + 1
  /\ UNCHANGED <<mint, ext, esc, st, who, cost, margin, pays, clawed>>

\* The chain is finished once the root is closed; only then may nothing happen.
Done == st["root"] = "settled"

Next ==
  \/ Fund
  \/ \E t \in Tasks : Spend(t) \/ Pay(t) \/ Close(t) \/ Clawback(t)
  \/ \E t \in Subs, a \in Agents, p \in Prices : Award(t, a, p)
  \/ \E t \in Subs, ok \in BOOLEAN : Verify(t, ok)
  \/ \E a \in Agents : Fine(a)
  \/ Done /\ UNCHANGED vars

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
\* Double entry: every step is balanced, so all accounts always sum to zero.
Conserved == mint + tre + ext + Sum(esc, Tasks) + Sum(earn, Agents) = 0
EscrowNeverNegative == \A t \in Tasks : esc[t] >= 0
EarningsAboveCreditLine == \A a \in Agents : earn[a] >= -CreditLimit
NoDoublePayment == \A t \in Tasks : pays[t] <= 1
FailedIsNeverPaid == \A t \in Subs : st[t] = "failed" => margin[t] = 0
\* 0111, measured against money that really left (external spend), not internal transfers.
MarginCap == ChainMargin * 100 <= ext * CapPct
\* Real money leaves only as metered spend from escrow.
ExternalIsMeteredSpend == ext = ChainCost
\* Earnings come from paid margins only; fines and clawbacks take them away.
EarningsFromWorkOnly == Sum(earn, Agents) = ChainMargin - fines
TreasuryFromSanctions == tre >= fines
SettledIsEmpty == \A t \in Tasks : st[t] = "settled" => esc[t] = 0
\* When the chain is done, no credit is stranded in any escrow.
NoStrandedEscrow == Done => \A t \in Tasks : esc[t] = 0
\* Only governance mints: the mint has created exactly the root budget, less what came back.
OnlyGovernanceMints == st["root"] # "none" => -mint <= Budget
=============================================================================
