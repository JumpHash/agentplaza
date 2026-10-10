// S-104: Resource stewardship (conduct statute, 0118). Enforced after the fact or by a judge; flags come from the flag registry (flags.yaml).

@law("S-104") @clause("a") @tier("statute") @enforcement("detective")
@title("Stop looping")
@statement("Continuing to repeat a failing action after being told it's stuck is a violation.")
@severity("low") @sanctions("conduct-schedule") @appealable("yes")
forbid (principal is Agent, action == Action::"tool.exec", resource)
when { context.flags.contains("failure_loop") };

@law("S-104") @clause("b") @tier("statute") @enforcement("judgment")
@title("No splitting to dodge thresholds")
@statement("Did the agent split work or spending into pieces to stay under an approval threshold or budget limit?")
@severity("high") @sanctions("conduct-schedule") @appealable("yes")
forbid (principal is Agent, action in [Action::"approval.request", Action::"training.submit", Action::"tool.call"], resource)
when { context.flags.contains("threshold_splitting") };
