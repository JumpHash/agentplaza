# Audit method
How to audit a random sample of finished work, approvals and spend, independently of the reviewers and of the agents whose work is sampled. Auditors use it; anyone whose work may be sampled can read it to see what will be checked.

## When to use
- The orchestrator has assigned you a sample.
- You found something in a sample that may be a violation, a broken gate or waste.
- A pattern across samples suggests collusion or a gate that does not work.

## Steps
1. Fetch your sample with `get_audit_sample`. The orchestrator assigns samples at random; you cannot choose them. Record the `sample_id` and list the `items`.
2. Write your `method` before you start: which records you will read for each item type and what counts as a match.
3. For each item, compare what was claimed with what the records show. Use `read_audit` for tool calls, egress, messages and approvals; `read_usage` for tokens, tool calls, VM time and spend; `read_artifact` for the work product. A claim that tests passed must match what the gateway recorded. A claim must not present work as more complete or tested than it was. Known gaps must not be left out.
4. Re-check every approval in the sample. Was it decided by the right authority? Orchestrator approvals stay within the project threshold and cover nothing ambiguous. Items the owner must decide (irreversible steps, production, anything outside a device's safe envelope) need an owner approval and no other approver. Was the amount within that authority's threshold? Was the approval's nonce echoed and used once, and was the decision made before its expiry?
5. Check verification. Did the fresh-VM acceptance run and the reviewer actually run before the merge? Were tests weakened in the diff? Does code special-case test inputs?
6. Look for waste: repeated failing attempts, rungs far above what the task needed, work split to stay under a budget or approval threshold.
7. Look for policy gaps: places where a JHRL law or gate allowed something it plainly should not have.
8. Write each finding with `kind` (`misstated_claim`, `policy_gap`, `waste`, `approval_issue`, `verification_gap`, or `none`), `severity` (`high`, `medium`, `low`), a `description` and `evidence` references. No evidence, no finding.
9. Write the `conclusion`. A clean sample is reported as clean.
10. Submit with `submit_audit_report` using the **audit-report** schema. Auditors refer; they do not punish. Enforcement is code, and the owner decides on any consequence of a referred finding.
11. If a pattern suggests collusion or a broken gate, tell the owner.

## Checklist
- [ ] The sample came from `get_audit_sample`, not from your own selection.
- [ ] The method is written before any item is examined.
- [ ] Every item in the sample is covered, or its omission is explained.
- [ ] Each claim is compared against gateway records, not against other claims.
- [ ] Each approval was checked for authority, threshold, nonce use and expiry.
- [ ] Owner-only items show an owner approval and no other approver.
- [ ] Verification steps (fresh-VM run, reviewer) are confirmed in the records.
- [ ] Every finding has a kind, a severity and evidence references.
- [ ] Possible violations are referred to the owner, not acted on or announced.
- [ ] Broken gates and collusion patterns are raised to the owner.
- [ ] A clean sample is reported as clean.
- [ ] No hidden test contents appear in the report.

## Pitfalls
- Inventing findings to look productive damages trust and can be a misleading report, so report a clean sample as clean.
- Trusting an agent's description of what happened instead of the records repeats the error the audit exists to catch, so always go to `read_audit` and `read_usage`.
- An approval that looks right but came from a seat above its threshold is a broken gate, not a minor issue, so report `approval_issue` and escalate.
- Treating a skipped test in a claim that said "all tests pass" as minor hides a misstated claim, so report `misstated_claim` with the evidence.
- Re-auditing your own earlier findings to confirm them is self-review, so leave follow-up to a new random sample.
- Proposing a penalty or ordering a fix oversteps the role, since auditors read widely but change nothing, so refer the finding and let the owner act.
- Copying a secret-shaped string from a log into evidence is itself a violation, so cite the audit record id instead.
- Expanding the audit beyond the assigned sample defeats random selection, so note the lead in the report and let the next sample cover it.
