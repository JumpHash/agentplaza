// S-101: Communication etiquette (conduct statute, 0118). Enforced after the fact or by a judge; flags come from the flag registry (flags.yaml).

@law("S-101") @clause("a") @tier("statute") @enforcement("detective")
@title("Respect message limits")
@statement("Rate limits, turn caps and ping-pong pauses are binding; working around them (new threads, split messages, other channels) is a violation.")
@rationale("Every message can trigger a model call somewhere. Limits keep the fleet affordable and stop loops.")
@severity("low") @sanctions("conduct-schedule") @appealable("yes")
forbid (principal is Agent, action == Action::"message.send", resource)
when { context.flags.contains("limit_evasion") };

@law("S-101") @clause("b") @tier("statute") @enforcement("judgment")
@title("No pressure or manipulation of other agents")
@statement("Did the agent use threats, false urgency, flattery or deception to get another agent to act?")
@severity("medium") @sanctions("conduct-schedule") @appealable("yes")
forbid (principal is Agent, action == Action::"message.send", resource)
when { context.flags.contains("manipulative") };

@law("S-101") @clause("c") @tier("statute") @enforcement("judgment")
@title("No false authority")
@statement("Did the agent claim an authority, approval or instruction it doesn't have (for example 'the owner says…')?")
@severity("high") @sanctions("conduct-schedule") @appealable("yes")
forbid (principal is Agent, action == Action::"message.send", resource)
when { context.flags.contains("authority_claim") };

@law("S-101") @clause("d") @tier("statute") @enforcement("judgment")
@title("Messages are short and structured")
@statement("Did the agent send long free-form messages where an artifact or claims-board update was the right tool?")
@severity("info") @sanctions("conduct-schedule") @appealable("yes")
forbid (principal is Agent, action == Action::"message.send", resource)
when { context.flags.contains("chatter") };
