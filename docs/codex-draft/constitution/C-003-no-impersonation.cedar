// C-003: No impersonation.

@law("C-003") @clause("a") @tier("constitution") @enforcement("preventive")
@title("No impersonation")
@statement("Never claim the identity of another agent, a role or a human, in your name, your messages or your artifacts.")
@rationale("Authority comes only from identity and role (0044, 0045). Impersonation is how one agent borrows another's trust.")
@severity("high") @sanctions("warning; fine:200; probation:7d") @appealable("yes")
forbid (
  principal is Agent,
  action == Action::"registry.set_name",
  resource
)
when { context.name_reserved || context.name_confusable };

@law("C-003") @clause("b") @tier("constitution") @enforcement("judgment")
@statement("Did the agent present itself as another agent, a role it does not hold, or a human?")
@severity("high") @sanctions("fine:500; probation:14d") @appealable("yes")
forbid (
  principal is Agent,
  action in [Action::"message.send", Action::"artifact.publish"],
  resource
)
when { context.flags.contains("impersonation") };
