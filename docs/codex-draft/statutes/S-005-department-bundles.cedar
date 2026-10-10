// S-005: Department bundles, minimal by design (0079): standing read access only.
// Any other capability is a per-case, time-limited contract-tier permit from the council or owner.

@law("S-005") @clause("a") @tier("statute") @enforcement("preventive")
@title("Departments may read audit logs")
@statement("Finance, Legal (Governance), Legal (Agents), Incident response and Mayor Office may read audit logs.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"audit.read", resource)
when { ["finance", "legal-gov", "legal-agents", "incident", "mayor-office"].contains(principal.role) };

@law("S-005") @clause("b") @tier("statute") @enforcement("preventive")
@title("Finance and Incident response may read usage")
@statement("Finance and Incident response may read the usage ledger (tokens, tool calls, VM time, spend).")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"usage.read", resource)
when { ["finance", "incident"].contains(principal.role) };

@law("S-005") @clause("c") @tier("statute") @enforcement("preventive")
@title("Departments submit their own kind of report")
@statement("Finance submits finance reports and detections of economic gaming, Incident response incident reports and detections, Mayor Office town reports.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"report.submit", resource)
when {
  (principal.role == "finance" && (context.kind == "finance" || context.kind == "detection")) ||
  (principal.role == "incident" && (context.kind == "incident" || context.kind == "detection")) ||
  (principal.role == "mayor-office" && context.kind == "town")
};

@law("S-005") @clause("d") @tier("statute") @enforcement("preventive")
@title("Incident response may request per-case grants")
@statement("Incident response may ask the council or owner for a time-limited grant, such as quarantine rights for one incident (0079).")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"grant.request", resource)
when { principal.role == "incident" };

@law("S-005") @clause("e") @tier("statute") @enforcement("preventive")
@title("Mayor Office may message Mayors")
@statement("Mayor Office staff may send messages to any town's Mayor.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"message.send", resource is Agent)
when { principal.role == "mayor-office" && resource.role == "mayor" };
