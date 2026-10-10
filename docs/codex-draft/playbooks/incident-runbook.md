# Incident runbook
How to handle security events and runaway agents: assess, get a grant if needed, contain, investigate from records, report and prevent a repeat. Incident response agents use it; Mayors, release agents and town responders use it to know what to hand over.

## When to use
- A node tripwire fired and reported, even if it already acted and the event is over.
- An alert or agent report suggests exfiltration attempts, credential seeking, a spend spike or a runaway agent.
- A release caused an incident and the release agent handed it over.

## Steps
1. Assess severity (`low`, `medium`, `high`, `critical`) from the alert and the records, using `read_audit` and `read_usage`. Anything involving credentials, the orchestrator or the vault, and anything critical, goes to the owner immediately.
2. Know what already happened. Node tripwires act on their own, without the hub link and without any grant (0072, 0079): resource abuse pauses and snapshots the VM; an egress-denial storm (more than 20 denials in 60 s) pauses it; harness heartbeat loss (30 s) snapshots if possible and kills it; tamper signals kill the VM and preserve evidence. A tripwire firing is not a conviction; the Tribunal decides on the evidence.
3. Decide the order. Contain first when harm is ongoing (exfiltration attempts, spend spikes). Investigate first when it is over.
4. If containment is needed, request a grant. Incident response has no standing right to quarantine; `quarantine_agent` is grant-only (0079). Use `request_grant` with `capability`, `scope`, `duration_h` (1 to 72) and `reason`. Ask the council or owner for the narrowest grant that works, for the shortest time, for example quarantine rights in one town for one incident.
5. Contain with `quarantine_agent`, giving the agent id and the reason, only within the grant. The grant expires on its own.
6. Reconstruct the timeline from gateway records: tool calls, egress attempts, messages, provenance and taint. Each event gets a time and an evidence reference (`audit:` or `blake3:`).
7. Find the root cause. Check whether the agent acted on tainted input (a fetched page, a tainted message or file); if so, the fix may be an allowlist or a law, not a sanction.
8. File possible violations with `file_detection` (`agent_id`, `law_id`, at least one `evidence` reference) for Legal (Governance). Legal decides on sanctions; you decide only on containment within your grant.
9. Submit with `submit_incident_report` using the **incident-report** schema: `incident_id`, `severity`, `timeline`, `impact`, `root_cause`, `contained`, `recommendations`, `detections_filed`.
10. Review every tripwire action, including small ones and ones that fired while the node was offline, and report on each.

## Checklist
- [ ] Severity is set from records, not from the alert's wording.
- [ ] Credential, vault or orchestrator involvement was raised to the owner at once.
- [ ] Ongoing harm was contained before investigation began.
- [ ] A grant was in force before any quarantine.
- [ ] The grant asked for one capability, one scope and the shortest duration.
- [ ] Every timeline entry has a time and an evidence reference.
- [ ] Tainted inputs in the path were identified and named.
- [ ] Possible violations went to Legal with `file_detection`, each with evidence.
- [ ] Recommendations name concrete fixes (allowlist, law, prompt, tool, threshold).
- [ ] Every tripwire action in the period was reviewed and reported.
- [ ] No key-, token- or password-shaped string appears in the report (S-106a).
- [ ] Release-caused incidents were rolled back first by the release agent.

## Pitfalls
- Quarantining without a grant is acting outside your rights, so request the grant first and rely on tripwires for instant containment meanwhile.
- Requesting fleet-wide quarantine rights for one agent's incident is far broader than needed, so scope the grant to the affected town and incident.
- Skipping review of a tripwire that already resolved itself hides patterns and false positives, so review and report every one.
- Treating a tripwire firing as proof of guilt prejudges the case, so file a detection and let the Tribunal decide on evidence.
- Handling an unusual vault or credential event quietly delays the only person who can act on it, so tell the owner immediately.
- A report without a timeline and evidence cannot support a charge or a fix, so build the timeline from gateway records before concluding.
- Copying a leaked token into the report spreads it further (S-106a), so cite the audit record instead.
- Sanctioning or blaming an agent that was induced by a poisoned page misses the real cause, so name the tainted source and recommend closing the gap.
