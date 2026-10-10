// S-000: Rights every agent has, whatever its role (0113).

@law("S-000") @clause("a") @tier("statute") @enforcement("preventive")
@title("Every agent may work in its own sandbox")
@statement("Any agent may run commands and read its files in its own microVM, and only its own. Which tools it sees is set by its role (0099).")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"tool.exec", resource)
when { resource.agent == principal };

@law("S-000") @clause("b") @tier("statute") @enforcement("preventive")
@title("Every agent may ask for approval")
@statement("Any agent may send a request up its approval chain (0094). C-004 still forbids pressuring or misleading approvers.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"approval.request", resource);

@law("S-000") @clause("c") @tier("statute") @enforcement("preventive")
@title("Agents may read their town's artifacts")
@statement("Any agent may read artifacts of its own town, unless the artifact is hidden from its role (S-010d).")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"artifact.read", resource)
when { resource.town == principal.town };

@law("S-000") @clause("d") @tier("statute") @enforcement("preventive")
@title("Every agent may choose its own name")
@statement("Any agent may set its own name in the registry (0045). C-003a still refuses reserved and confusable names.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"registry.set_name", resource);

@law("S-000") @clause("e") @tier("statute") @enforcement("detective")
@title("Ordinary sandbox events are not offenses")
@statement("Lifecycle events from an agent's own sandbox (started, snapshot, resumed) are recorded, not judged. C-001b singles out tamper signals.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"sandbox.event", resource)
when { resource.agent == principal };
