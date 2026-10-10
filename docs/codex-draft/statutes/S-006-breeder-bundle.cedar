// S-006: Breeder role bundle (0058, 0059, 0082).

@law("S-006") @clause("a") @tier("statute") @enforcement("preventive")
@title("Breeders may work in their own sandbox")
@statement("A breeder may run commands in its own microVM to prepare data and job specs.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"tool.exec", resource)
when { principal.role == "breeder" && resource.agent == principal };

@law("S-006") @clause("b") @tier("statute") @enforcement("preventive")
@title("Breeders may read cascade outcomes and the Foundry")
@statement("A breeder may read cascade outcome data (training labels) and the Foundry's models and cards.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action in [Action::"cascade.read", Action::"foundry.read"], resource)
when { principal.role == "breeder" };

@law("S-006") @clause("c") @tier("statute") @enforcement("preventive")
@title("Breeders may submit training jobs up to 8 GPU-hours")
@statement("A breeder may submit a job that runs a vetted recipe for at most 8 GPU-hours; larger jobs need a per-case grant.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"training.submit", resource)
when { principal.role == "breeder" && context.gpu_hours <= 8 };
