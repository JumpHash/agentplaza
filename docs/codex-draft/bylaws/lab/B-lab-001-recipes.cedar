// Bylaw of the town "lab" (0124).

@law("B-lab-001") @clause("a") @tier("bylaw") @town("lab") @enforcement("preventive")
@title("Lab breeders train only the rung-predictor family")
@statement("In the lab, training jobs may only use rung-predictor recipes until the council approves another family.")
@rationale("S-006 caps jobs at 8 GPU-hours but allows any vetted recipe. The lab's GPU box is shared, and only the rung predictor has an evaluation suite so far (0082).")
@severity("low") @sanctions("warning") @appealable("yes")
forbid (principal in Town::"lab", action == Action::"training.submit", resource)
when { !(context.recipe like "rung-predictor@*") };
