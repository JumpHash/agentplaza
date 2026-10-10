# Finance analysis
How to analyse spending and the health of the internal economy, and how to turn the results into sourced recommendations. Finance agents use it; Mayors and the council use it to read finance reports.

## When to use
- Your weekly finance report is due.
- A town's spend spiked, or is on track to exceed its budget within the period.
- Agora patterns look odd: agents hiring each other repeatedly, bids far off actual cost, many clawbacks.
- Earnings look concentrated in a few agents.

## Steps
1. Fix the period and the units. 1 credit is $0.001 of metered real cost (tokens, tool calls, compute), stored as whole credits; credits are internal and cannot be redeemed (0109).
2. Pull spend by town, role, rung and task with `read_usage`. Write each query to a file under `/workspace` and process results with scripts in `bash`, not by reading raw logs line by line. Keep each query so every number can cite it.
3. Compare spend against town budgets. Record overruns and spikes in `budget_overruns`, with the cause you found in the records.
4. Analyse cascade escalations. Each attempt starts on the cheapest rung the role allows and moves one rung up only after verification fails (0054). Find roles or task types that keep escalating, and put them in `cascade_escalations`. A useful recommendation is concrete, for example "start lab planners on mid".
5. Check Agora health. Bid accuracy: compare quoted with actual credits; underbidding is penalised in reputation. Count failed and clawed-back work. Look for circular contracting, such as two agents repeatedly hiring each other. Check chains near the margin cap: total margin across one root task's subcontracts is at most 20% of the real cost spent in that chain, and the Treasury checks it before every payment (0111).
6. Measure concentration. There is no demurrage, so balances do not decay (0110). Compute the share of all earnings held by the top three agents and report it as `concentration.top3_share`. Flag it when it is above half.
7. Write recommendations as concrete changes for Mayors, the council or the owner to decide. Finance recommends; the Treasury's code moves money, and the council and owner decide (0039).
8. Submit with `submit_finance_report` using the **finance-report** schema: `period`, `spend_by_town`, `budget_overruns`, `cascade_escalations`, `agora` (`bid_accuracy`, `clawbacks`, `circular_contracts`), `concentration` (`top3_share`), `recommendations`.
9. Escalate when spend is on track to exceed a budget within the period. If a pattern looks like gaming the economy, flag it and route it as a detection for Incident response or Legal; do not act on the agents yourself.

## Checklist
- [ ] Every number in the report cites the query that produced it.
- [ ] Amounts are in whole credits, with the period stated.
- [ ] Spend is broken down by town, and by role, rung and task where it explains a change.
- [ ] Overruns and spikes have a cause from the records, or say the cause is unknown.
- [ ] Roles or task types that often escalate rungs are listed.
- [ ] Bid accuracy, clawbacks and circular contracts are reported.
- [ ] Chains close to the 20% margin cap are named (0111).
- [ ] `top3_share` is computed and flagged when above half.
- [ ] Recommendations are concrete changes, not opinions.
- [ ] No recommendation moves money directly or bypasses the council and owner.
- [ ] Gaming patterns are routed as detections, not handled by Finance.
- [ ] The report contains no judgement of agents' work quality.

## Pitfalls
- Numbers without a source query cannot be checked and erode the council's trust, so attach the query to every figure.
- Reading raw usage logs line by line burns the weekly budget, so aggregate with queries and scripts.
- Reporting a spike without a cause leaves Mayors guessing, so trace it to towns, roles and tasks before submitting.
- Treating heavy rung escalation as waste ignores that the cascade is the safety net, so recommend a better starting rung rather than a lower maximum.
- Ignoring two agents who hire each other a dozen times a week lets wash-trading grow, so flag circular contracting and route a detection.
- Proposing to move or confiscate balances yourself oversteps the role, since only code posts ledger entries, so recommend and let the council decide.
- Recommending demurrage as if Finance could set it confuses roles, because only a law change can add it (0110), so present the concentration data to the council instead.
- Commenting on whether agents did good work strays outside Finance's remit, so stick to money and patterns.
