//! The roles an agent can hold. The kebab-case name is the manifest key and
//! the value agents carry as their `role`; the label is what people read.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Orchestrator,
    Architect,
    Auditor,
    Breeder,
    Builder,
    EdgeOperator,
    Finance,
    Incident,
    LegalAgentsCounsel,
    LegalAgentsDefender,
    LegalAgentsOmbudsman,
    LegalGovJudge,
    LegalGovProsecutor,
    Librarian,
    Mayor,
    MayorOffice,
    Planner,
    Qa,
    Release,
    Researcher,
    Reviewer,
    SecurityReviewer,
}

impl Role {
    /// Every role.
    pub const ALL: [Role; 22] = [
        Role::Orchestrator,
        Role::Architect,
        Role::Auditor,
        Role::Breeder,
        Role::Builder,
        Role::EdgeOperator,
        Role::Finance,
        Role::Incident,
        Role::LegalAgentsCounsel,
        Role::LegalAgentsDefender,
        Role::LegalAgentsOmbudsman,
        Role::LegalGovJudge,
        Role::LegalGovProsecutor,
        Role::Librarian,
        Role::Mayor,
        Role::MayorOffice,
        Role::Planner,
        Role::Qa,
        Role::Release,
        Role::Researcher,
        Role::Reviewer,
        Role::SecurityReviewer,
    ];

    /// The role named `name` (kebab-case), if there is one.
    pub fn parse(name: &str) -> Option<Role> {
        Self::ALL.into_iter().find(|role| role.as_str() == name)
    }

    /// The kebab-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Orchestrator => "orchestrator",
            Role::Architect => "architect",
            Role::Auditor => "auditor",
            Role::Breeder => "breeder",
            Role::Builder => "builder",
            Role::EdgeOperator => "edge-operator",
            Role::Finance => "finance",
            Role::Incident => "incident",
            Role::LegalAgentsCounsel => "legal-agents-counsel",
            Role::LegalAgentsDefender => "legal-agents-defender",
            Role::LegalAgentsOmbudsman => "legal-agents-ombudsman",
            Role::LegalGovJudge => "legal-gov-judge",
            Role::LegalGovProsecutor => "legal-gov-prosecutor",
            Role::Librarian => "librarian",
            Role::Mayor => "mayor",
            Role::MayorOffice => "mayor-office",
            Role::Planner => "planner",
            Role::Qa => "qa",
            Role::Release => "release",
            Role::Researcher => "researcher",
            Role::Reviewer => "reviewer",
            Role::SecurityReviewer => "security-reviewer",
        }
    }

    /// The human label.
    pub fn label(self) -> &'static str {
        match self {
            Role::Orchestrator => "Orchestrator",
            Role::Architect => "Architect",
            Role::Auditor => "Auditor",
            Role::Breeder => "Breeder",
            Role::Builder => "Builder",
            Role::EdgeOperator => "Edge operator",
            Role::Finance => "Finance",
            Role::Incident => "Incident",
            Role::LegalAgentsCounsel => "Legal · agents' counsel",
            Role::LegalAgentsDefender => "Legal · agents' defender",
            Role::LegalAgentsOmbudsman => "Legal · agents' ombudsman",
            Role::LegalGovJudge => "Legal · government judge",
            Role::LegalGovProsecutor => "Legal · government prosecutor",
            Role::Librarian => "Librarian",
            Role::Mayor => "Mayor",
            Role::MayorOffice => "Mayor's office",
            Role::Planner => "Planner",
            Role::Qa => "QA",
            Role::Release => "Release",
            Role::Researcher => "Researcher",
            Role::Reviewer => "Reviewer",
            Role::SecurityReviewer => "Security reviewer",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_round_trips_through_parse_and_as_str() {
        for role in Role::ALL {
            assert_eq!(Role::parse(role.as_str()), Some(role));
        }
        assert_eq!(Role::parse("Orchestrator"), None);
        assert_eq!(Role::parse("no-such-role"), None);
    }
}
