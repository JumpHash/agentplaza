// Example contract-tier policy: a per-case grant (0079). Issued by the council or owner for one incident,
// time-limited, scoped to one town. The hub fills context.now_unix. This file is an example, not a law in force.

@law("G-0042") @clause("a") @tier("contract") @enforcement("preventive")
@title("Quarantine rights for incident inc-4")
@statement("Agent ivo may quarantine agents of town shop until the grant expires.")
@severity("info") @sanctions("none") @appealable("no")
permit (principal == Agent::"ivo", action == Action::"agent.quarantine", resource)
when { resource.town == Town::"shop" && context.now_unix < 1791500000 };
