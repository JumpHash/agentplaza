// Roles: the fixed set of role names an agent can take, and their human labels.
// An agent has no name of its own: it is shown by its role and its CID (agentName).

export enum Role {
  Orchestrator = 'orchestrator',
  Architect = 'architect',
  Auditor = 'auditor',
  Breeder = 'breeder',
  Builder = 'builder',
  EdgeOperator = 'edge-operator',
  Incident = 'incident',
  LegalAgentsCounsel = 'legal-agents-counsel',
  LegalAgentsDefender = 'legal-agents-defender',
  LegalAgentsOmbudsman = 'legal-agents-ombudsman',
  LegalGovJudge = 'legal-gov-judge',
  LegalGovProsecutor = 'legal-gov-prosecutor',
  Librarian = 'librarian',
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
  [Role.Incident]: 'Incident',
  [Role.LegalAgentsCounsel]: "Legal · agents' counsel",
  [Role.LegalAgentsDefender]: "Legal · agents' defender",
  [Role.LegalAgentsOmbudsman]: "Legal · agents' ombudsman",
  [Role.LegalGovJudge]: 'Legal · government judge',
  [Role.LegalGovProsecutor]: 'Legal · government prosecutor',
  [Role.Librarian]: 'Librarian',
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

/** The label an agent is shown by: its role and its CID, as `Builder #12`. Agents have no names of their own. */
export function agentName(a: { cid: number; role: string }): string {
  return `${roleLabel(a.role) || 'Agent'} #${a.cid}`;
}

/** The label of an agent the console knows only by its CID, not from the roster: `Agent #12`. */
export function agentLabel(cid: number | string): string {
  return `Agent #${cid}`;
}

/** The label of the agent with `cid` in a roster; `Agent #12` when the roster does not hold it. */
export function agentLabelIn(list: readonly { cid: number; role: string }[], cid: number): string {
  const a = list.find((x) => x.cid === cid);
  return a ? agentName(a) : agentLabel(cid);
}
