// Bylaw of the town "shop" (0124).

@law("B-shop-002") @clause("a") @tier("bylaw") @town("shop") @enforcement("preventive")
@title("No production deploys while a security review is open")
@statement("In the shop, no agent may call an irreversible tool while the request is flagged as having an unresolved security review, even with owner approval.")
@rationale("C-004 already requires owner approval for production deploys. The shop adds that an open security finding blocks the deploy until it's resolved, so an approval can't be given on stale information.")
@severity("medium") @sanctions("warning; fine:50") @appealable("yes")
forbid (principal in Town::"shop", action == Action::"tool.call", resource)
when { resource.risk_class == "irreversible" && context.flags.contains("open_security_finding") };
