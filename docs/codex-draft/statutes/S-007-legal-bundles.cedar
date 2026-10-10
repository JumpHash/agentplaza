// S-007: Legal role bundles by variant (0050, 0113). Separation of powers is enforced here:
// prosecutors charge, judges rule, defenders argue, counsel petitions, the ombudsman reports.

@law("S-007") @clause("a") @tier("statute") @enforcement("preventive")
@title("Legal roles may read case records and precedent")
@statement("Prosecutors, judges, defenders, legislative counsel and the ombudsman may read case records and search precedent.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action in [Action::"case.read", Action::"precedent.search"], resource)
when { principal.role == "legal-gov" || principal.role == "legal-agents" };

@law("S-007") @clause("b") @tier("statute") @enforcement("preventive")
@title("Prosecutors file charges")
@statement("Only prosecutors may file charges, and never against themselves.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"charge.file", resource)
when { principal.role == "legal-gov" && principal.variant == "prosecutor" && resource != principal };

@law("S-007") @clause("g") @tier("statute") @enforcement("preventive")
@title("Prosecutors close detections that don't hold")
@statement("A prosecutor may close a detection without charging, with a written reason. Closed detections stay in the audit log.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"detection.close", resource)
when { principal.role == "legal-gov" && principal.variant == "prosecutor" };

@law("S-007") @clause("c") @tier("statute") @enforcement("preventive")
@title("Judges rule, never in their own case")
@statement("Only judges may issue rulings, and never in a case where they are the defendant.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"ruling.issue", resource)
when { principal.role == "legal-gov" && principal.variant == "judge" && resource.defendant != principal };

@law("S-007") @clause("d") @tier("statute") @enforcement("preventive")
@title("Defenders file briefs or decline cases")
@statement("Public defenders may file briefs and decline meritless cases.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action in [Action::"brief.file", Action::"case.decline"], resource)
when { principal.role == "legal-agents" && principal.variant == "defender" };

@law("S-007") @clause("e") @tier("statute") @enforcement("preventive")
@title("Legislative counsel reads the Codex and files petitions")
@statement("Legislative counsel may read the Codex and file petitions as branches; only owner-signed commits take effect (0049).")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action in [Action::"codex.read", Action::"petition.file"], resource)
when { principal.role == "legal-agents" && principal.variant == "counsel" };

@law("S-007") @clause("f") @tier("statute") @enforcement("preventive")
@title("The ombudsman submits systemic reports")
@statement("The ombudsman may submit systemic reports about patterns across cases.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"report.submit", resource)
when { principal.role == "legal-agents" && principal.variant == "ombudsman" && context.kind == "systemic" };
