// S-004: Mayor role bundle (0032: LLM agent acting only through hub APIs scoped to its town).

@law("S-004") @clause("a") @tier("statute") @enforcement("preventive")
@title("Mayors assign work within their town")
@statement("A Mayor may assign its town's tasks to agents of its own town.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"task.assign", resource)
when {
  principal.role == "mayor" &&
  resource.town == principal.town &&
  context.assignee.town == principal.town
};

@law("S-004") @clause("b") @tier("statute") @enforcement("preventive")
@title("Mayors manage their town's population")
@statement("A Mayor may spawn and hibernate agents in its own town, within the town's quota (quota enforced by the scheduler).")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action in [Action::"agent.spawn", Action::"agent.hibernate"], resource)
when { principal.role == "mayor" && resource == principal.town };

@law("S-004") @clause("c") @tier("statute") @enforcement("preventive")
@title("Mayors approve plans and merges below threshold")
@statement("A Mayor may approve plans and merges for its town's tasks when the budget is within the town's threshold and nothing is ambiguous; everything else goes to the owner (0053, 0067).")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action in [Action::"plan.approve", Action::"merge.approve"], resource)
when {
  principal.role == "mayor" &&
  resource.town == principal.town &&
  context.budget <= context.threshold &&
  !context.ambiguous
};

@law("S-004") @clause("d") @tier("statute") @enforcement("preventive")
@statement("A Mayor may message agents of its own town.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"message.send", resource is Agent)
when { principal.role == "mayor" && resource.town == principal.town };

@law("S-004") @clause("e") @tier("statute") @enforcement("preventive")
@title("Mayors may message the Mayor Office")
@statement("A Mayor may send messages to Mayor Office staff, who support all Mayors and carry town issues to the council.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"message.send", resource is Agent)
when { principal.role == "mayor" && resource.role == "mayor-office" };
