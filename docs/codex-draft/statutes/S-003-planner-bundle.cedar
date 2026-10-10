// S-003: Planner role bundle (0053: planner turns specs into task graphs).

@law("S-003") @clause("a") @tier("statute") @enforcement("preventive")
@title("Planners may work in their own sandbox and read the hub's git")
@statement("A planner may run commands in its own microVM and reach the hub's git (specs and repos).")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"tool.exec", resource)
when { principal.role == "planner" && resource.agent == principal };

@law("S-003") @clause("b") @tier("statute") @enforcement("preventive")
@statement("A planner may connect to the hub-git allowlist.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"egress.connect", resource is Host)
when { principal.role == "planner" && resource in HostGroup::"hub-git" };

@law("S-003") @clause("c") @tier("statute") @enforcement("preventive")
@title("Planners may create tasks in their town")
@statement("A planner may create tasks in its own town. Tasks still need plan approval before work starts.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"task.create", resource)
when { principal.role == "planner" && resource == principal.town };

@law("S-003") @clause("d") @tier("statute") @enforcement("preventive")
@title("Planners may submit plans for their town")
@statement("A planner may submit a plan for its own town; it runs only after the plan gate (Mayor or owner).")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"plan.submit", resource)
when { principal.role == "planner" && resource == principal.town };
