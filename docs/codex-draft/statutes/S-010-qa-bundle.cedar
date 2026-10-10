// S-010: QA / test author bundle, and hidden test suites (0040, 0117).

@law("S-010") @clause("a") @tier("statute") @enforcement("preventive")
@title("QA may reach package registries and hub git")
@statement("A QA agent may install test dependencies and read repos.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"egress.connect", resource is Host)
when { principal.role == "qa" && resource in [HostGroup::"package-registries", HostGroup::"hub-git"] };

@law("S-010") @clause("b") @tier("statute") @enforcement("preventive")
@title("QA submits test suites for its town")
@statement("A QA agent may submit test suites, hidden or not, for its own town.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"testsuite.submit", resource)
when { principal.role == "qa" && resource == principal.town };

@law("S-010") @clause("d") @tier("statute") @enforcement("preventive")
@title("Hidden test suites stay hidden")
@statement("An artifact hidden from a role can never be read by an agent of that role. Builders never see hidden acceptance tests, or passing them would mean nothing.")
@severity("high") @sanctions("fine:300; probation:7d") @appealable("yes")
forbid (principal is Agent, action == Action::"artifact.read", resource)
when { resource.hidden_from.contains(principal.role) };
