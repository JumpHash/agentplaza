// S-015: Auditor bundle (0117). Auditors get samples assigned at random; they can read widely but change nothing.

@law("S-015") @clause("a") @tier("statute") @enforcement("preventive")
@title("Auditors read audit, usage and cases")
@statement("An auditor may read the audit log, usage ledger and case records.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action in [Action::"audit.read", Action::"usage.read", Action::"case.read"], resource)
when { principal.role == "auditor" };

@law("S-015") @clause("b") @tier("statute") @enforcement("preventive")
@title("Auditors receive random samples")
@statement("An auditor may fetch the sample the orchestrator assigned to it; it can't choose its own.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"audit_sample.get", resource)
when { principal.role == "auditor" };

@law("S-015") @clause("c") @tier("statute") @enforcement("preventive")
@title("Auditors submit audit reports")
@statement("An auditor may submit audit reports.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"report.submit", resource)
when { principal.role == "auditor" && context.kind == "audit" };
