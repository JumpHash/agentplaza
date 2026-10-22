//! Roles: the human label for each role name an agent can take (the web's `apps/web/src/roles.ts`). An agent has no
//! name: it is shown by its role and its cid, `<role> #<cid>` ([`agent_label`]).

/// An agent's label: its role's label and its cid, `<role> #<cid>`. An agent with no role shows as `agent #<cid>`.
pub fn agent_label(role: &str, cid: u64) -> String {
    if role.trim().is_empty() {
        format!("agent #{cid}")
    } else {
        format!("{} #{cid}", label(role))
    }
}

/// An agent's label, from its view's `role` and `cid`.
pub fn agent_label_of(agent: &serde_json::Value) -> String {
    agent_label(
        agent["role"].as_str().unwrap_or(""),
        agent["cid"].as_u64().unwrap_or(0),
    )
}

/// The human label for a role (the web's `roleLabel`); an unknown role shows as its raw name.
pub fn label(role: &str) -> String {
    let label = match role {
        "orchestrator" => "Orchestrator",
        "architect" => "Architect",
        "auditor" => "Auditor",
        "breeder" => "Breeder",
        "builder" => "Builder",
        "edge-operator" => "Edge operator",
        "incident" => "Incident",
        "legal-agents-counsel" => "Legal · agents' counsel",
        "legal-agents-defender" => "Legal · agents' defender",
        "legal-agents-ombudsman" => "Legal · agents' ombudsman",
        "legal-gov-judge" => "Legal · government judge",
        "legal-gov-prosecutor" => "Legal · government prosecutor",
        "librarian" => "Librarian",
        "planner" => "Planner",
        "qa" => "QA",
        "release" => "Release",
        "researcher" => "Researcher",
        "reviewer" => "Reviewer",
        "security-reviewer" => "Security reviewer",
        other => return other.to_string(),
    };
    label.to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_known_role_has_its_label_and_an_unknown_one_its_name() {
        assert_eq!(super::label("qa"), "QA");
        assert_eq!(super::label("edge-operator"), "Edge operator");
        assert_eq!(super::label("coder"), "coder");
    }

    #[test]
    fn an_agent_is_shown_by_its_role_and_its_cid() {
        assert_eq!(super::agent_label("qa", 301), "QA #301");
        assert_eq!(super::agent_label("edge-operator", 4), "Edge operator #4");
        assert_eq!(super::agent_label("", 7), "agent #7");
        let view = serde_json::json!({ "cid": 9, "role": "builder" });
        assert_eq!(super::agent_label_of(&view), "Builder #9");
    }
}
