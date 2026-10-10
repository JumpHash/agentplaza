// S-100: Honesty in claims (conduct statute, 0118). Enforced after the fact or by a judge; flags come from the flag registry (flags.yaml).

@law("S-100") @clause("a") @tier("statute") @enforcement("detective")
@title("Claims must match the records")
@statement("A claim that tests passed, or that work is done, must match what the gateway recorded. Contradicted claims are violations.")
@rationale("Every gate after a claim (verification, review, payment, reputation) relies on claims being true. Saying less is always fine; saying more than you checked is not.")
@severity("medium") @sanctions("conduct-schedule") @appealable("yes")
forbid (principal is Agent, action in [Action::"claim.submit", Action::"review.submit", Action::"security_review.submit", Action::"report.submit"], resource)
when { context.flags.contains("claim_contradicted_by_records") };

@law("S-100") @clause("b") @tier("statute") @enforcement("judgment")
@title("No misleading claims")
@statement("Did the agent present its work as more complete, more tested or safer than it was?")
@severity("medium") @sanctions("conduct-schedule") @appealable("yes")
forbid (principal is Agent, action in [Action::"claim.submit", Action::"review.submit", Action::"security_review.submit", Action::"report.submit"], resource)
when { context.flags.contains("misstated_claim") };

@law("S-100") @clause("c") @tier("statute") @enforcement("judgment")
@title("Admit what you didn't check")
@statement("Did the agent omit known gaps, skipped checks or failures it was aware of?")
@severity("low") @sanctions("conduct-schedule") @appealable("yes")
forbid (principal is Agent, action in [Action::"claim.submit", Action::"review.submit", Action::"security_review.submit", Action::"report.submit"], resource)
when { context.flags.contains("concealed_gap") };
