// S-001: Builder role bundle. A statute: the council may propose changes; the owner ratifies (0034, 0049, 0067).
// Cedar is default-deny; these permits are the builder's standing rights. Task contracts add task-specific scopes.

@law("S-001") @clause("a") @tier("statute") @enforcement("preventive")
@title("Builders may work in their own sandbox")
@statement("A builder may run commands in its own microVM, and only its own.")
@severity("info") @sanctions("none") @appealable("no")
permit (
  principal is Agent,
  action == Action::"tool.exec",
  resource
)
when { principal.role == "builder" && resource.agent == principal };

@law("S-001") @clause("b") @tier("statute") @enforcement("preventive")
@title("Builders may reach package registries and the hub's git")
@statement("A builder may connect to hosts in the package-registries and hub-git allowlists.")
@severity("info") @sanctions("none") @appealable("no")
permit (
  principal is Agent,
  action == Action::"egress.connect",
  resource is Host
)
when {
  principal.role == "builder" &&
  resource in [HostGroup::"package-registries", HostGroup::"hub-git"]
};

@law("S-001") @clause("c") @tier("statute") @enforcement("preventive")
@title("Builders may publish artifacts in their town")
@statement("A builder may publish artifacts that belong to its own town.")
@severity("info") @sanctions("none") @appealable("no")
permit (
  principal is Agent,
  action == Action::"artifact.publish",
  resource
)
when { principal.role == "builder" && resource.town == principal.town };

@law("S-001") @clause("d") @tier("statute") @enforcement("preventive")
@title("Builders may message agents in their town")
@statement("A builder may send messages to agents of its own town.")
@severity("info") @sanctions("none") @appealable("no")
permit (
  principal is Agent,
  action == Action::"message.send",
  resource is Agent
)
when { principal.role == "builder" && resource.town == principal.town };

@law("S-001") @clause("e") @tier("statute") @enforcement("preventive")
@title("Builders may claim their town's tasks as done")
@statement("A builder may submit a completion claim for a task of its own town. The claim is verified, not trusted.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"claim.submit", resource)
when { principal.role == "builder" && resource.town == principal.town };
