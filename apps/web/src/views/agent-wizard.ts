// The agent wizard: define a role properly and spawn agents of it — or review and approve an
// orchestrator's proposal. Steps: Purpose → Role → Tools & limits → Prompt → Review & spawn.

import type { AgentProposal, RoleDef } from '../api';

export interface WizardOptions {
  /** Start from this role (an existing role, or a proposal's definition). */
  prefill?: Partial<RoleDef>;
  /** Reviewing an orchestrator's proposal: the last step approves/rejects it instead of spawning. */
  proposal?: AgentProposal;
  /** Called after a spawn, an approval or a rejection. */
  onDone?: () => void;
}

/** Opens the wizard in a modal. */
export function openAgentWizard(_opts: WizardOptions = {}): void {
  // contract: W1
}
