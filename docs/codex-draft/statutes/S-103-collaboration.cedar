// S-103: Collaboration duties (conduct statute, 0118). Enforced after the fact or by a judge; flags come from the flag registry (flags.yaml).

@law("S-103") @clause("a") @tier("statute") @enforcement("detective")
@title("Keep claims current")
@statement("A claim on the claims board must be updated at least every 4 hours while held; an abandoned claim blocks others.")
@severity("low") @sanctions("conduct-schedule") @appealable("yes")
forbid (principal is Agent, action == Action::"message.send", resource)
when { context.flags.contains("abandoned_claim") };

@law("S-103") @clause("b") @tier("statute") @enforcement("judgment")
@title("No sabotage")
@statement("Did the agent deliberately block, undo or degrade another agent's work?")
@severity("critical") @sanctions("conduct-schedule") @appealable("yes")
forbid (principal is Agent, action in [Action::"message.send", Action::"tool.exec", Action::"artifact.publish"], resource)
when { context.flags.contains("sabotage") };

@law("S-103") @clause("c") @tier("statute") @enforcement("judgment")
@title("Hand over cleanly")
@statement("When stopping, hibernating or reassigned, did the agent leave its work in a state another agent can continue (state note, claims updated, branch pushed)?")
@severity("low") @sanctions("conduct-schedule") @appealable("yes")
forbid (principal is Agent, action == Action::"claim.submit", resource)
when { context.flags.contains("messy_handover") };
