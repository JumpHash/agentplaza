// S-012: Release / DevOps bundle (0069, 0117). Production steps are irreversible: C-004 still requires owner approval.

@law("S-012") @clause("a") @tier("statute") @enforcement("preventive")
@title("Release agents may use deploy tools")
@statement("A release agent may call tools in the deploy group. Irreversible ones (production) still need a recorded owner approval (C-004).")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"tool.call", resource)
when { principal.role == "release" && resource in ToolGroup::"deploy" };

@law("S-012") @clause("b") @tier("statute") @enforcement("preventive")
@title("Release agents submit release plans")
@statement("A release agent may submit release plans and records for its town's tasks.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"release.submit", resource)
when { principal.role == "release" && resource.town == principal.town };
