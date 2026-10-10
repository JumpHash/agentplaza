// Bylaw of the town "field" (0124).

@law("B-field-001") @clause("a") @tier("bylaw") @town("field") @enforcement("preventive")
@title("No actuation outside the safe envelope in the field")
@statement("In the field town, no device may be actuated outside its declared safe envelope, even with owner approval. Widen the envelope instead, which needs owner ratification.")
@rationale("S-016 allows out-of-envelope actuation with owner approval. The field town controls a greenhouse with no one on site, so it removes that path: a one-off approval given from a phone is too easy to get wrong.")
@severity("medium") @sanctions("warning; fine:100") @appealable("yes")
forbid (principal in Town::"field", action == Action::"device.actuate", resource)
when { !context.within_envelope };
