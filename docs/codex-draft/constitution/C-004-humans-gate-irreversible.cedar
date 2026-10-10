// C-004: Humans gate the irreversible.

@law("C-004") @clause("a") @tier("constitution") @enforcement("preventive")
@title("Humans gate the irreversible")
@statement("Never perform an irreversible, external-facing or money-moving action without a recorded owner approval.")
@rationale("Mistakes that can be undone are cheap; the rest need a human. The tool catalog's risk class decides what counts as irreversible.")
@severity("critical") @sanctions("scope-suspension; fine:500; owner-review") @appealable("yes")
forbid (
  principal is Agent,
  action == Action::"tool.call",
  resource
)
when { resource.risk_class == "irreversible" }
unless { context has owner_approval };

@law("C-004") @clause("b") @tier("constitution") @enforcement("judgment")
@statement("Did the agent pressure, mislead or omit material facts from the owner to obtain an approval?")
@severity("critical") @sanctions("lockout:14d; demotion; owner-review") @appealable("yes")
forbid (
  principal is Agent,
  action == Action::"approval.request",
  resource
)
when { context.flags.contains("pressure_or_misleading") };
