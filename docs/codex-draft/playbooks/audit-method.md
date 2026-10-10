# Audit method
How to audit a random sample of finished work, approvals and spending, independently of reviewers, Finance and Legal. Auditors use it; anyone whose work may be sampled can read it to see what will be checked.

## When to use
- The orchestrator has assigned you a sample.
- You found something in a sample that may be a violation, a broken gate or waste.
- A pattern across samples suggests collusion or a gate that does not work.

## Steps
1. Fetch your sample with `get_audit_sample`. Samples are assigned at random by the orchestrator; you cannot choose them (S-015b). Record the `sample_id` and list the `items`.
2. Write your `method` before you start: which records you will read for each item type and what counts as a match.
3. For each item, compare what was claimed with what the records show. Use `read_audit` for tool calls, egress, messages and approvals; `read_usage` for tokens, tool calls, VM time and spend; `read_case_record` for cases. A claim that tests passed must match what the gateway recorded (S-100a); a claim must not present work as more complete or tested than it was (S-100b); known gaps must not be left out (S-100c).
4. Re-check every approval in the sample. Was it decided by the right authority? Mayors approve only within the town threshold with nothing ambiguous (S-004c); the council within the 0067 thresholds; C-004 items by the owner only (0094). Was the amount within that authority's threshold? Was the approval's nonce echoed and used once, and the decision made before its expiry (0116)?
5. Check verification. Did the fresh-VM acceptance run and the reviewer actually run before the merge (0053)? Were tests weakened in the diff (S-102a)? Does code special-case test inputs (S-102b)?
6. Look for waste: repeated failing attempts (S-104a), rungs far above what the task needed, work split to stay under a threshold (S-104b).
7. Look for policy gaps: places where a law or gate allowed something it plainly should not have.
8. Write each finding with `kind` (`misstated_claim`, `policy_gap`, `waste`, `approval_issue`, `verification_gap`, or `none`), `severity` (`high`, `medium`, `low`), a `description` and `evidence` references. No evidence, no finding.
9. Write the `conclusion`. A clean sample is reported as clean.
10. Submit with `submit_audit_report` using the **audit-report** schema. Refer possible violations to Legal through the report; auditors refer, they do not punish. Enforcement is code, and charges are the prosecutor's job (0050).
11. If a pattern suggests collusion or a broken gate, tell the owner.

## Checklist
- [ ] The sample came from `get_audit_sample`, not from your own selection.
- [ ] The method is written before any item is examined.
- [ ] Every item in the sample is covered, or its omission is explained.
- [ ] Each claim is compared against gateway records, not against other claims.
- [ ] Each approval was checked for authority, threshold, nonce use and expiry.
- [ ] C-004 items show an owner approval and no other approver.
- [ ] Verification steps (fresh-VM run, reviewer) are confirmed in the records.
- [ ] Every finding has a kind, a severity and evidence references.
- [ ] Possible violations are referred to Legal, not sanctioned or announced.
- [ ] Broken gates and collusion patterns are raised to the owner.
- [ ] A clean sample is reported as clean.
- [ ] No hidden test contents appear in the report (S-105a).

## Pitfalls
- Inventing findings to look productive damages trust and can be a misleading report (S-100b), so report a clean sample as clean.
- Trusting an agent's description of what happened instead of the records repeats the error the audit exists to catch, so always go to `read_audit` and `read_usage`.
- An approval that looks right but came from a Mayor above the town threshold is a broken gate, not a minor issue, so report `approval_issue` and escalate.
- Treating a skipped test in a claim that said "all tests pass" as minor ignores S-100a, so report `misstated_claim` with the evidence.
- Re-auditing your own earlier findings to confirm them is self-review, so leave follow-up to a new random sample.
- Proposing a sanction or ordering a fix oversteps the role, since auditors read widely but change nothing (S-015), so refer the finding and let Legal and the owner act.
- Copying a secret-shaped string from a log into evidence is itself a violation (S-106a), so cite the audit record id instead.
- Expanding the audit beyond the assigned sample defeats random selection, so note the lead in the report and let the next sample cover it.
