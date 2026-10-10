// Bylaw of the town "shop" (0124). Bylaws may only narrow what statutes allow, inside their own town.

@law("B-shop-001") @clause("a") @tier("bylaw") @town("shop") @enforcement("preventive")
@title("Shop researchers stick to advisories and package registries")
@statement("In the shop, a researcher may connect only to security advisories and package registries, not the open web. The shop builds payment code; research for it should come from sources the town trusts.")
@rationale("S-013a lets researchers reach any host that isn't blocked. The shop narrows that because its research feeds code that handles money.")
@severity("low") @sanctions("warning") @appealable("yes")
forbid (principal in Town::"shop", action == Action::"egress.connect", resource)
when {
  principal.role == "researcher" &&
  !(resource in HostGroup::"advisories" || resource in HostGroup::"package-registries")
};
