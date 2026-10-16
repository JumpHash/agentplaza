//! The tabs, in order, with their groups, and which apply to a scope. A tab that does not apply is hidden, not
//! shown empty.

/// (key, label, group).
pub const TABS: &[(&str, &str, &str)] = &[
    ("overview", "Overview", "Health"),
    ("alerts", "Alerts", "Health"),
    ("tokens", "Tokens & spend", "Models"),
    ("context", "Context", "Models"),
    ("latency", "Latency", "Models"),
    ("traces", "Traces", "Models"),
    ("agents", "Agents & work", "Models"),
    ("compute", "Compute", "Fleet"),
    ("lifecycle", "Lifecycle", "Fleet"),
    ("storage", "Storage & IO", "Fleet"),
    ("hub", "Hub", "Fleet"),
    ("capacity", "Capacity & forecast", "Plan"),
    ("events", "Events", ""),
    ("dash", "My dashboard", ""),
];

/// Whether a tab applies to a scope (`kind` is the scope kind prefix: global, node, run, vm, ws, sb, agent,
/// role, provider, model).
pub fn applies(tab: &str, kind: &str, scope_id: &str) -> bool {
    match tab {
        "hub" => kind == "global" || scope_id == "node:n0",
        "compute" | "lifecycle" | "storage" => !matches!(kind, "provider" | "model"),
        "agents" => !matches!(kind, "vm" | "ws" | "sb"),
        _ => TABS.iter().any(|t| t.0 == tab),
    }
}

/// The label of a tab key.
pub fn label(tab: &str) -> &'static str {
    TABS.iter()
        .find(|t| t.0 == tab)
        .map(|t| t.1)
        .unwrap_or("Overview")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fleet_tabs_hide_for_providers_and_the_hub_tab_shows_only_at_global_and_on_n0() {
        assert!(!applies("compute", "provider", "provider:anthropic"));
        assert!(applies("compute", "node", "node:n1"));
        assert!(applies("hub", "node", "node:n0"));
        assert!(!applies("hub", "node", "node:n1"));
        assert!(!applies("agents", "vm", "vm:r12/1"));
        assert!(!applies("nonsense", "global", "global"));
    }
}
