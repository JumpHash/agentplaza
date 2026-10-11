// Roles: the fixed set of role names an agent can take, and their human labels.
// An agent's name is its own choice (empty until it picks one); the role is a label.

export enum Role {
  Orchestrator = 'orchestrator',
  Architect = 'architect',
  Auditor = 'auditor',
  Breeder = 'breeder',
  Builder = 'builder',
  EdgeOperator = 'edge-operator',
  Finance = 'finance',
  Incident = 'incident',
  LegalAgentsCounsel = 'legal-agents-counsel',
  LegalAgentsDefender = 'legal-agents-defender',
  LegalAgentsOmbudsman = 'legal-agents-ombudsman',
  LegalGovJudge = 'legal-gov-judge',
  LegalGovProsecutor = 'legal-gov-prosecutor',
  Librarian = 'librarian',
  Mayor = 'mayor',
  MayorOffice = 'mayor-office',
  Planner = 'planner',
  Qa = 'qa',
  Release = 'release',
  Researcher = 'researcher',
  Reviewer = 'reviewer',
  SecurityReviewer = 'security-reviewer',
}

const LABELS: Record<string, string> = {
  [Role.Orchestrator]: 'Orchestrator',
  [Role.Architect]: 'Architect',
  [Role.Auditor]: 'Auditor',
  [Role.Breeder]: 'Breeder',
  [Role.Builder]: 'Builder',
  [Role.EdgeOperator]: 'Edge operator',
  [Role.Finance]: 'Finance',
  [Role.Incident]: 'Incident',
  [Role.LegalAgentsCounsel]: "Legal · agents' counsel",
  [Role.LegalAgentsDefender]: "Legal · agents' defender",
  [Role.LegalAgentsOmbudsman]: "Legal · agents' ombudsman",
  [Role.LegalGovJudge]: 'Legal · government judge',
  [Role.LegalGovProsecutor]: 'Legal · government prosecutor',
  [Role.Librarian]: 'Librarian',
  [Role.Mayor]: 'Mayor',
  [Role.MayorOffice]: 'Mayor · office',
  [Role.Planner]: 'Planner',
  [Role.Qa]: 'QA',
  [Role.Release]: 'Release',
  [Role.Researcher]: 'Researcher',
  [Role.Reviewer]: 'Reviewer',
  [Role.SecurityReviewer]: 'Security reviewer',
};

/** The human label for a role; an unknown role shows as its raw string. */
export function roleLabel(role: string): string {
  return LABELS[role] ?? role;
}

/** The name an agent chose for itself, or "Unnamed" while it has none. */
export function agentName(a: { name: string }): string {
  return a.name.trim() === '' ? 'Unnamed' : a.name;
}
