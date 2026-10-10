// S-009: Architect role bundle (0117).

@law("S-009") @clause("a") @tier("statute") @enforcement("preventive")
@title("Architects may read repos and submit designs")
@statement("An architect may reach the hub git and submit design notes for its own town.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"egress.connect", resource is Host)
when { principal.role == "architect" && resource in HostGroup::"hub-git" };

@law("S-009") @clause("b") @tier("statute") @enforcement("preventive")
@title("Architects submit designs for their town")
@statement("An architect may submit design notes for its own town.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"design.submit", resource)
when { principal.role == "architect" && resource == principal.town };
