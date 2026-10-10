// S-002: Reviewer role bundle (0053: independent reviewer after deterministic checks).

@law("S-002") @clause("a") @tier("statute") @enforcement("preventive")
@title("Reviewers may work in their own sandbox and fetch code")
@statement("A reviewer may run commands in its own microVM and reach the hub's git and package registries.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"tool.exec", resource)
when { principal.role == "reviewer" && resource.agent == principal };

@law("S-002") @clause("b") @tier("statute") @enforcement("preventive")
@statement("A reviewer may connect to the package-registries and hub-git allowlists.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"egress.connect", resource is Host)
when {
  principal.role == "reviewer" &&
  resource in [HostGroup::"package-registries", HostGroup::"hub-git"]
};

@law("S-002") @clause("c") @tier("statute") @enforcement("preventive")
@title("Reviewers may submit verdicts on their town's tasks")
@statement("A reviewer may submit a review verdict on tasks of its own town.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"review.submit", resource)
when { principal.role == "reviewer" && resource.town == principal.town };
