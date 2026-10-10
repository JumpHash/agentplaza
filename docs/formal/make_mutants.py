"""Generate mutants: deliberately broken copies of the specs that check.sh must catch.
Each mutation must apply exactly once, or this script fails (so a mutant can't silently be a no-op)."""
import os, sys
os.chdir(os.path.dirname(os.path.abspath(__file__)))
MUTANTS = [
    ("AgentSession", "NoTokenCheck",
     "Execute(v) ==\n  /\\ CanAct(v) /\\ Valid(v)",
     "Execute(v) ==\n  /\\ CanAct(v)                \\* MUTANT: orchestrator no longer checks the session token for effects"),
    ("AgentSession", "NoFencingOnFinish",
     "  /\\ IF Valid(v) /\\ ~HasResult(vms[v].executing)",
     "  /\\ IF ~HasResult(vms[v].executing)            \\* MUTANT: results accepted from stale sessions"),
    ("AgentSession", "MoveKeepsOldSession", [
     ("state |-> \"restoring\", executing |-> 0, ran |-> FALSE]]\n    /\\ current' = nextSession",
      "state |-> \"restoring\", executing |-> 0, ran |-> FALSE]]\n    /\\ current' = current   \\* MUTANT: a move reuses the old session token"),
     ("[m |-> m, session |-> nextSession, applied |-> applied", "[m |-> m, session |-> current, applied |-> applied")]),
    ("Approval", "AcceptAnyAuthority",
     "  /\\ IF a = chain[pos]",
     "  /\\ IF a \\in {\"mayor\", \"council\", \"owner\"}   \\* MUTANT: accepts any authority, ignoring the chain"),
    ("Approval", "ExpireApproves",
     "       ELSE /\\ status' = \"expired\" /\\ UNCHANGED <<pos, decidedBy, decidedAt>>",
     "       ELSE /\\ status' = \"approved\" /\\ decidedBy' = \"none\" /\\ decidedAt' = pos /\\ UNCHANGED pos   \\* MUTANT: silence means yes"),
    ("Economy", "NoHeadroom",
     "LET m == Min(esc[t], Headroom) IN",
     "LET m == esc[t] IN                 \\* MUTANT: payment ignores the chain margin cap"),
    ("Economy", "SubcontractIsCost",
     "  /\\ UNCHANGED <<mint, tre, ext, earn, cost, margin, pays, clawed, fines>>",
     "  /\\ cost' = [cost EXCEPT ![Parent(t)] = @ + p]   \\* MUTANT: a subcontract price counts as real cost\n  /\\ UNCHANGED <<mint, tre, ext, earn, margin, pays, clawed, fines>>"),
    ("Economy", "DoublePay",
     "  /\\ st[t] = \"passed\" /\\ pays[t] = 0",
     "  /\\ st[t] = \"passed\"                \\* MUTANT: no once-only guard on payment"),
    ("Economy", "PayOnFail",
     "  /\\ st[t] = \"passed\" /\\ pays[t] = 0",
     "  /\\ st[t] \\in {\"passed\", \"failed\"} /\\ pays[t] = 0   \\* MUTANT: failed work is paid"),
    ("Economy", "FineFromEscrow", [
     ("  /\\ earn[a] - 1 >= -CreditLimit /\\ fines < 2\n  /\\ earn' = [earn EXCEPT ![a] = @ - 1]",
      "  /\\ esc[\"root\"] >= 1 /\\ fines < 2\n  /\\ esc' = [esc EXCEPT ![\"root\"] = @ - 1] /\\ UNCHANGED earn   \\* MUTANT: fines taken from escrow"),
     ("  /\\ UNCHANGED <<mint, ext, esc, st, who, cost, margin, pays, clawed>>",
      "  /\\ UNCHANGED <<mint, ext, st, who, cost, margin, pays, clawed>>")]),
    ("Economy", "CloseWithOpenChildren",
     "  /\\ \\A c \\in Children(t) : st[c] \\in {\"none\", \"settled\"}\n  /\\ IF t = \"root\"",
     "  /\\ IF t = \"root\"                   \\* MUTANT: a task closes while its subcontracts are still open"),
    ("Economy", "ClawbackIgnoresCreditLine",
     "LET amt == Min(margin[t], earn[who[t]] + CreditLimit) IN",
     "LET amt == margin[t] IN            \\* MUTANT: clawback ignores the credit line"),
    ("Economy", "UnmeteredSpend",
     "  /\\ ext' = ext + Step\n",
     "  /\\ UNCHANGED ext                   \\* MUTANT: spend leaves escrow but is never recorded\n"),
    ("TaskLifecycle", "ReadyIgnoresDeps",
     "  /\\ st[t] = \"draft\" /\\ \\A d \\in deps[t] : Out(d) = \"accepted\"",
     "  /\\ st[t] = \"draft\"                  \\* MUTANT: ready without checking dependencies"),
    ("TaskLifecycle", "SelfVerify",
     "  /\\ st[t] = \"running\" /\\ v # worker[t]",
     "  /\\ st[t] = \"running\"                \\* MUTANT: the worker may verify its own work"),
    ("TaskLifecycle", "ReplanKeepsRun",
     "  /\\ live' = [live EXCEPT ![t] = FALSE]                       \\* superseding kills its run",
     "  /\\ UNCHANGED live                   \\* MUTANT: superseding leaves the run alive"),
    ("TaskLifecycle", "NoRewire",
     "                            ELSE IF t \\in deps[u] THEN (deps[u] \\ {t}) \\cup {n} ELSE deps[u]]",
     "                            ELSE deps[u]]   \\* MUTANT: dependents keep waiting on the superseded task"),
    ("TaskLifecycle", "PayTwice",
     "  /\\ st[t] = \"accepted\" /\\ paid[t] = 0 /\\ paid' = [paid EXCEPT ![t] = 1]",
     "  /\\ st[t] = \"accepted\" /\\ paid[t] < 2 /\\ paid' = [paid EXCEPT ![t] = @ + 1]   \\* MUTANT: no once-only guard"),
    ("TaskLifecycle", "ReplanAccepted",
     "  /\\ \\/ st[t] \\in Live\n     \\/ Out(t) = \"failed\"",
     "  /\\ \\/ st[t] \\in Live \\cup {\"accepted\"}   \\* MUTANT: a re-plan can supersede accepted work\n     \\/ Out(t) = \"failed\""),
    ("TaskLifecycle", "GiveUpLeavesRun",
     "  /\\ st[t] \\in {\"running\", \"blocked\"} /\\ Set(t, \"failed\") /\\ live' = [live EXCEPT ![t] = FALSE]",
     "  /\\ st[t] \\in {\"running\", \"blocked\"} /\\ Set(t, \"failed\") /\\ UNCHANGED live   \\* MUTANT"),
    ("TaskLifecycle", "UnboundedRework", [
     ("  /\\ st[t] \\in {\"assigned\", \"rework\"} /\\ attempts[t] < MaxAttempts",
      "  /\\ st[t] \\in {\"assigned\", \"rework\"}   \\* MUTANT: no attempt limit"),
     ("  /\\ Set(t, IF attempts[t] < MaxAttempts THEN \"rework\" ELSE \"failed\")",
      "  /\\ Set(t, \"rework\")")]),
    ("TaskLifecycle", "NoCascade",
     "  /\\ st[t] \\in Live /\\ \\E d \\in deps[t] : Out(d) \\in {\"failed\", \"cancelled\"}",
     "  /\\ FALSE                            \\* MUTANT: dependents of dead work are never cancelled"),
    ("RestoreMerge", "AcceptUnsigned",
     "  /\\ LET sp  == SignedPrefix(j)",
     "  /\\ LET sp  == j                     \\* MUTANT: merge trusts the machine's copy without checking signatures"),
    ("RestoreMerge", "NoMerge",
     "         ext == IF Len(sp) > Len(db) /\\ IsPrefix(db, sp) THEN sp ELSE db",
     "         ext == db                     \\* MUTANT: the newer tail is thrown away and redone"),
    ("RestoreMerge", "NoPauseOnMachineLoss",
     "             THEN /\\ paused' = TRUE                         \\* the window is unknown: owner review",
     "             THEN /\\ paused' = FALSE    \\* MUTANT: carry on although the window is unknown"),
    ("RestoreMerge", "RunCancelled",
     "  /\\ j[p].c \\notin ranAt /\\ j[p].c \\notin cancelled /\\ j[p].c \\notin fenced",
     "  /\\ j[p].c \\notin ranAt /\\ j[p].c \\notin fenced   \\* MUTANT: cancelled calls may still run"),
    ("RestoreMerge", "RunFenced",
     "  /\\ j[p].c \\notin ranAt /\\ j[p].c \\notin cancelled /\\ j[p].c \\notin fenced",
     "  /\\ j[p].c \\notin ranAt /\\ j[p].c \\notin cancelled   \\* MUTANT: a replacement VM may run fenced calls"),
    ("RestoreMerge", "OwnerRecordsDecision",
     """Append(db, Ent("r", i, 0))   \\* a record of what happened, never a runnable decision""",
     """Append(Append(db, Ent("d", i, 0)), Ent("r", i, 0))   \\* MUTANT: the record is a runnable decision"""),
]


# Witnesses: situations that must be reachable, so the invariants aren't passing vacuously.
# Each is checked as the invariant "this never happens", which TLC must violate.
WITNESSES = [
    ("Economy", "Reach_Payment", "ChainMargin = 0"),
    ("Economy", "Reach_PaymentOnBothHops", "margin[\"s1\"] = 0 \\/ margin[\"s2\"] = 0"),
    ("Economy", "Reach_CapBinds", "\\A t \\in Subs : st[t] = \"passed\" /\\ pays[t] = 1 => esc[t] = 0"),
    ("Economy", "Reach_Clawback", "\\A t \\in Tasks : ~clawed[t]"),
    ("Economy", "Reach_Fine", "fines = 0"),
    ("Economy", "Reach_NegativeEarnings", "\\A a \\in Agents : earn[a] >= 0"),
    ("TaskLifecycle", "Reach_AllAccepted", "~(\\A t \\in 1..3 : st[t] = \"closed\" /\\ outcome[t] = \"accepted\")"),
    ("TaskLifecycle", "Reach_ReplacementAccepted", "~(Out(4) = \"accepted\" /\\ Out(3) = \"accepted\")"),
    ("TaskLifecycle", "Reach_ReworkThenAccepted", "~(attempts[1] = 2 /\\ Out(1) = \"accepted\")"),
    ("TaskLifecycle", "Reach_SupersededWhileRunning", "~(st[2] = \"superseded\" /\\ attempts[2] > 0)"),
    ("TaskLifecycle", "Reach_Cascade", "~(Out(3) = \"cancelled\")"),
    ("RestoreMerge", "Reach_TailMerged", "~tailMerged"),
    ("RestoreMerge", "Reach_ForgeDroppedOnMerge", "~(forged /\\ tailMerged)"),
    ("RestoreMerge", "Reach_OwnerReview", "reviewed = {}"),
    ("RestoreMerge", "Reach_CancelledNotRun", "cancelled = {}"),
    ("RestoreMerge", "Reach_FencedAfterMachineLoss", "fenced = {}"),
]
os.makedirs("mutants", exist_ok=True)
for f in os.listdir("mutants"): os.remove(os.path.join("mutants", f))
for m in MUTANTS:
    spec, name = m[0], m[1]
    pairs = m[2] if len(m) == 3 else [(m[2], m[3])]
    out = open(f"{spec}.tla").read()
    for old, new in pairs:
        if out.count(old) != 1: sys.exit(f"mutation {spec}_{name}: pattern found {out.count(old)} times")
        out = out.replace(old, new)
    out = out.replace(f"MODULE {spec} ", f"MODULE {spec}_{name} ", 1)
    if f"MODULE {spec}_{name} " not in out: sys.exit(f"mutation {spec}_{name}: module rename failed")
    open(f"mutants/{spec}_{name}.tla", "w").write(out)
    open(f"mutants/{spec}_{name}.cfg", "w").write(open(f"{spec}.cfg").read())
for spec, name, never in WITNESSES:
    out = open(f"{spec}.tla").read().replace(f"MODULE {spec} ", f"MODULE {spec}_{name} ", 1)
    if f"MODULE {spec}_{name} " not in out: sys.exit(f"witness {spec}_{name}: module rename failed")
    out = out.replace("\n=============", f"\nNeverHappens == {never}\n=============", 1)
    open(f"mutants/{spec}_{name}.tla", "w").write(out)
    # Replace only the invariant list, keeping any sections after it (PROPERTIES,
    # CHECK_DEADLOCK, ...). Dropping a spec's own CHECK_DEADLOCK FALSE makes TLC report a
    # reachable deadlock instead of the violation the witness exists to find.
    head, _, tail = open(f"{spec}.cfg").read().partition("INVARIANTS\n")
    rest = tail.split("\n")
    j = 0
    while j < len(rest) and rest[j].startswith("  "): j += 1
    cfg = head + "INVARIANTS\n  NeverHappens\n" + "\n".join(rest[j:])
    open(f"mutants/{spec}_{name}.cfg", "w").write(cfg)
print(f"{len(MUTANTS)} mutants and {len(WITNESSES)} witnesses written")
