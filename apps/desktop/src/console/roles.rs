//! Roles: the human label for each role name an agent can take (the web's `apps/web/src/roles.ts`). An agent's
//! name is its own choice; the role is a label.

/// The human label for a role (the web's `roleLabel`); an unknown role shows as its raw name.
pub fn label(role: &str) -> String {
    let label = match role {
        "orchestrator" => "Orchestrator",
        "architect" => "Architect",
        "auditor" => "Auditor",
        "breeder" => "Breeder",
        "builder" => "Builder",
        "edge-operator" => "Edge operator",
        "finance" => "Finance",
        "incident" => "Incident",
        "legal-agents-counsel" => "Legal · agents' counsel",
        "legal-agents-defender" => "Legal · agents' defender",
        "legal-agents-ombudsman" => "Legal · agents' ombudsman",
        "legal-gov-judge" => "Legal · government judge",
        "legal-gov-prosecutor" => "Legal · government prosecutor",
        "librarian" => "Librarian",
        "mayor" => "Mayor",
        "mayor-office" => "Mayor · office",
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
}
