// S-011: Security reviewer bundle (0105, 0117).

@law("S-011") @clause("a") @tier("statute") @enforcement("preventive")
@title("Security reviewers may reach registries, advisories and hub git")
@statement("A security reviewer may fetch dependencies, advisory databases and repos.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"egress.connect", resource is Host)
when { principal.role == "security-reviewer" && resource in [HostGroup::"package-registries", HostGroup::"advisories", HostGroup::"hub-git"] };

@law("S-011") @clause("b") @tier("statute") @enforcement("preventive")
@title("Security reviewers submit verdicts on their town's tasks")
@statement("A security reviewer may submit a security review for tasks of its own town.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"security_review.submit", resource)
when { principal.role == "security-reviewer" && resource.town == principal.town };
