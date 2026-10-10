// S-008: Council seats (0034). Seats are held by the heads of Finance, Legal (Agents),
// Incident response and Mayor Office; Legal (Governance) never sits (judges excluded).

@law("S-008") @clause("a") @tier("statute") @enforcement("preventive")
@title("Council members propose motions and vote")
@statement("Agents holding a council seat may propose motions and cast votes. Judges and prosecutors never hold seats.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action in [Action::"motion.propose", Action::"vote.cast"], resource)
when { principal.council_seat && principal.role != "legal-gov" };
