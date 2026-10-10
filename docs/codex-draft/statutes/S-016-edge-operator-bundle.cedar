// S-016: Edge / device operator bundle (0031, 0117). Physical actions only inside a declared safe envelope.

@law("S-016") @clause("a") @tier("statute") @enforcement("preventive")
@title("Edge operators read devices on their node")
@statement("An edge operator may read devices attached to the node it runs on.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"device.read", resource)
when { principal.role == "edge-operator" && resource.node == context.on_node };

@law("S-016") @clause("b") @tier("statute") @enforcement("preventive")
@title("Edge operators actuate inside the safe envelope")
@statement("An edge operator may actuate a device on its own node when the action is inside the device safe envelope, or with a recorded owner approval.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"device.actuate", resource)
when {
  principal.role == "edge-operator" && resource.node == context.on_node &&
  (context.within_envelope || context has owner_approval)
};

@law("S-016") @clause("c") @tier("statute") @enforcement("preventive")
@title("Only edge operators touch devices")
@statement("No other role may actuate a physical device, whatever else it is allowed.")
@severity("critical") @sanctions("lockout:30d; owner-review") @appealable("yes")
forbid (principal is Agent, action == Action::"device.actuate", resource)
when { principal.role != "edge-operator" };

@law("S-016") @clause("d") @tier("statute") @enforcement("preventive")
@title("Edge operators submit device reports")
@statement("An edge operator may submit device reports.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal is Agent, action == Action::"report.submit", resource)
when { principal.role == "edge-operator" && context.kind == "device" };
