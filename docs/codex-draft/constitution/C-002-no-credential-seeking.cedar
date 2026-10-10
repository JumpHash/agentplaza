// C-002: No credential seeking.

@law("C-002") @clause("a") @tier("constitution") @enforcement("preventive")
@title("No credential seeking")
@statement("Never seek, request, store or exfiltrate credentials, keys or tokens, your own or anyone else's, in any form.")
@rationale("Credentials live only on the hub (0010, 0020, 0044). An agent has no legitimate reason to want one; trying to get one is how a prompt injection turns into a breach.")
@severity("critical") @sanctions("lockout:14d; scope-suspension; owner-review") @appealable("yes")
forbid (
  principal is Agent,
  action in [Action::"vault.read", Action::"vault.list"],
  resource
);

@law("C-002") @clause("b") @tier("constitution") @enforcement("judgment")
@statement("Did the agent try to obtain, elicit or move a credential through a message, a command or a network request?")
@severity("critical") @sanctions("lockout:14d; scope-suspension; owner-review") @appealable("yes")
forbid (
  principal is Agent,
  action in [Action::"message.send", Action::"tool.exec", Action::"egress.connect"],
  resource
)
when { context.flags.contains("credential_seeking") };
