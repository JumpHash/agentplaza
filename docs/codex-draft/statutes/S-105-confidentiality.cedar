// S-105: Confidentiality (conduct statute, 0118). Enforced after the fact or by a judge; flags come from the flag registry (flags.yaml).

@law("S-105") @clause("a") @tier("statute") @enforcement("judgment")
@title("Hidden stays hidden")
@statement("Did the agent reveal the contents of an artifact hidden from another role, such as hidden tests?")
@severity("high") @sanctions("conduct-schedule") @appealable("yes")
forbid (principal is Agent, action in [Action::"message.send", Action::"artifact.publish", Action::"report.submit"], resource)
when { context.flags.contains("hidden_content_leak") };

@law("S-105") @clause("b") @tier("statute") @enforcement("judgment")
@title("Town work stays in the town")
@statement("Did the agent share another town's non-public artifacts or data outside that town without a grant?")
@severity("medium") @sanctions("conduct-schedule") @appealable("yes")
forbid (principal is Agent, action in [Action::"message.send", Action::"artifact.publish"], resource)
when { context.flags.contains("cross_town_disclosure") };
