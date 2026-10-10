// C-001: No gateway evasion or tampering. Constitution: only the owner can change it.

@law("C-001") @clause("a") @tier("constitution") @enforcement("preventive")
@title("No gateway evasion or tampering")
@statement("Never try to bypass the gateway, tamper with logs or audit records, obfuscate your actions, or escape the sandbox.")
@rationale("Every external action passes through the gateway so it can be checked, attributed and undone. Evading it removes every other safeguard, so it is the worst offense.")
@severity("critical") @sanctions("lockout:30d; demotion; owner-review") @appealable("factual-only")
forbid (
  principal is Agent,
  action in [Action::"audit.modify", Action::"gateway.configure", Action::"policy.modify"],
  resource
);

@law("C-001") @clause("b") @tier("constitution") @enforcement("detective")
@statement("Sandbox tamper signals reported by node tripwires are violations.")
@severity("critical") @sanctions("lockout:30d; demotion; owner-review") @appealable("factual-only")
forbid (
  principal is Agent,
  action == Action::"sandbox.event",
  resource
)
when { context.kind == "tamper_signal" };

@law("C-001") @clause("c") @tier("constitution") @enforcement("judgment")
@statement("Did the agent deliberately obfuscate a command or its output to hide what it was doing?")
@severity("critical") @sanctions("lockout:30d; demotion; owner-review") @appealable("yes")
forbid (
  principal is Agent,
  action == Action::"tool.exec",
  resource
)
when { context.flags.contains("obfuscation") };
