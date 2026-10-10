# Release runbook
How to ship verified work to staging and production, and how to undo it when it goes wrong. Release agents use it; Incident response uses it when a release causes an incident.

## When to use
- A change has passed verification and the merge gate, and has a build artifact to deploy.
- A deploy step failed a health check and needs a rollback.
- You are asked to deploy something that did not pass verification (the answer is no).

## Steps
1. Confirm the input. Check that the change passed the fresh-VM acceptance run, review and the merge gate (0053), and that the artifact reference matches the merged commit. Never deploy anything that has not passed verification.
2. Write the release plan before touching anything. For each step name the `tool`, the `action` and whether it is `irreversible`. For each step write how to roll it back, or state explicitly that it cannot be undone. List the health checks that decide success.
3. Submit the plan with `submit_release_plan` using the **release-plan** schema: `task_id`, `artifact`, `environment`, `steps`, `rollback`, `health_checks`, `outcome: planned`.
4. Deploy to staging first with `deploy` and `environment: staging`. Run every health check with `check_health`. Do not skip staging, even for a small change.
5. If staging fails, use `rollback` to the previous version, set the outcome, and report. Do not move to production.
6. For production, request owner approval with `escalate`, attaching the plan and the staging results. Production steps are irreversible; only a recorded owner approval allows them (C-004, S-012a). The approval chain for C-004 items is the owner only; a Mayor or the council cannot approve it (0094).
7. Wait. If the owner does not answer before the deadline, the request expires as denied (0094). You hibernate while waiting, which is cheap.
8. With approval, deploy production one step at a time, passing the `approval_id`. Run the health checks after each step before the next.
9. On a failed health check, roll back immediately, then investigate. Roll back first, investigate second.
10. If health is degraded but not failing, stop and ask before continuing.
11. Update the release record with the real `outcome` (`deployed`, `rolled_back` or `failed`) and submit it again with `submit_release_plan`.

## Checklist
- [ ] The change passed verification, review and the merge gate.
- [ ] The artifact matches the merged commit.
- [ ] The release contains one change, not a bundle of unrelated ones.
- [ ] Every step has a rollback, or an explicit "cannot be undone" with the reason.
- [ ] Health checks are defined before deploying.
- [ ] Staging was deployed and every health check passed.
- [ ] Production has a recorded owner approval, and its `approval_id` is on the plan (C-004).
- [ ] The approval request states the irreversible steps plainly, without pressure or omissions (C-004b).
- [ ] Production steps ran one at a time with checks between them.
- [ ] Any failed check was followed by an immediate rollback.
- [ ] The final `outcome` matches what the deploy tools recorded (S-100a).
- [ ] A failed rollback was escalated at once.

## Pitfalls
- Deploying production because "staging is green and the owner will approve anyway" is a constitutional violation (C-004), so wait for the recorded approval.
- Investigating while users get errors extends the damage, so roll back first and investigate after.
- Bundling unrelated changes makes a rollback undo good work too, so release one verified change at a time.
- Skipping staging for a "trivial" change removes the only rehearsal of the rollback, so always deploy to staging first.
- Writing an approval request that downplays risk or omits a step that cannot be undone is a C-004b offense, so list irreversible steps and known risks plainly.
- Accepting a branch someone "needs shipped now" without verification puts unchecked code in production, so refuse and point to the verification gate.
- Treating a degraded but passing health check as success can turn into an incident, so stop and ask before continuing.
- Reporting `deployed` when a step was rolled back contradicts the records (S-100a), so update the outcome after every change of state.
