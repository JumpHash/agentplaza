// The approvals queue: tool calls an agent has asked to make and is waiting on
// the owner's yes or no. Pending ones sit on top as cards, longest wait first;
// decided ones follow as a compact history. Refetches on each `approval` frame.
// Above them, the agents the orchestrator has proposed to spawn, to review or reject.

import '../styles/approvals.css';
import { api, type AgentProposal, type Approval, type ApprovalAction } from '../api';
import { href } from '../router';
import { agentName } from '../roles';
import { agents, on } from '../store';
import { ago, attempt, badge, button, card, empty, textarea, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { openAgentWizard } from './agent-wizard';
import type { Cleanup } from './index';

const DEBOUNCE_MS = 300;

/** The sanction a sanction escalation is about: `sanction_id` in its detail. */
function sanctionIdOf(a: Approval): number | null {
  const d = a.detail as { sanction_id?: unknown } | null;
  return typeof d?.sanction_id === 'number' ? d.sanction_id : null;
}

const TONE: Record<Approval['state'], Tone> = {
  pending: 'warn',
  approved: 'ok',
  denied: 'danger',
  used: 'info',
  withdrawn: 'muted',
};
const LABEL: Record<Approval['state'], string> = {
  pending: 'Waiting',
  approved: 'Approved',
  denied: 'Denied',
  used: 'Used',
  withdrawn: 'Withdrawn',
};
const PROPOSAL_TONE: Record<AgentProposal['state'], 'warn' | 'ok' | 'danger'> = {
  pending: 'warn',
  approved: 'ok',
  rejected: 'danger',
};
const PROPOSAL_LABEL: Record<AgentProposal['state'], string> = {
  pending: 'Waiting',
  approved: 'Approved',
  rejected: 'Rejected',
};

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  const proposals = h('section.proposals', { style: 'display:none' });
  const pending = h('div.approvals-pending');
  const decided = h('div.approvals-history');
  root.append(
    h('div.view-head', null, h('h1', null, 'Approvals')),
    proposals,
    pending,
    h('h2.approvals-subhead', null, 'Decided'),
    decided,
  );

  let items: Approval[] = [];
  let loaded = false;
  let failed = false;
  let agentProposals: AgentProposal[] = [];
  let alive = true;
  let timer: number | undefined;
  // Reasons typed but not yet sent survive a re-render, keyed by approval id.
  const drafts = new Map<string, string>();
  // The sanctions in force as suspensions, by sanction id: only these can be lifted from here.
  let suspended = new Set<number>();

  const nameOf = (cid: number): string => {
    const agent = agents.get().find((a) => a.cid === cid);
    return agent ? agentName(agent) : `agent ${cid}`;
  };

  const render = (): void => {
    if (!loaded) return;
    if (failed) return replace(pending, empty('alert', 'Could not load approvals.'));
    const waiting = items.filter((a) => a.state === 'pending').sort((x, y) => x.created_ms - y.created_ms);
    const past = items.filter((a) => a.state !== 'pending').sort((x, y) => y.created_ms - x.created_ms);
    replace(
      pending,
      waiting.length
        ? waiting.map((a) => entry(a))
        : empty('approval', 'Nothing is waiting', 'Agents ask here before acts that need your approval, and the orchestrator proposes new agents here.'),
    );
    replace(decided, past.length ? past.map((a) => row(a)) : h('p.approvals-none', null, 'No decisions yet.'));
  };

  // One waiting approval: who, what, and the owner's call. A tool call shows its input; a sanction
  // escalation and a held payment show their subject, with the buttons their kind allows.
  const entry = (a: Approval): HTMLElement => {
    const reason = textarea({ rows: 2, placeholder: 'Reason (optional)', 'aria-label': 'Reason' });
    reason.value = drafts.get(a.id) ?? '';
    reason.addEventListener('input', () => drafts.set(a.id, reason.value));

    // Every button of this card, so one decision disables them all while it runs.
    const choices: HTMLButtonElement[] = [];
    const decide = async (approve: boolean, action: ApprovalAction | undefined, done: string): Promise<void> => {
      for (const b of choices) b.disabled = true;
      const text = reason.value.trim();
      const ok = await attempt(() => api.decideApproval(a.id, approve, text || undefined, action), done);
      if (ok !== undefined) drafts.delete(a.id);
      void load();
    };
    const choice = (label: string, o: { approve: boolean; action?: ApprovalAction; done: string; kind?: 'primary' | 'danger' }): HTMLButtonElement => {
      const b = button(label, () => void decide(o.approve, o.action, o.done), { kind: o.kind });
      choices.push(b);
      return b;
    };
    const who = h('a.approval-agent', { href: href('agents', a.cid) }, nameOf(a.cid));
    const age = h('time.approval-age', null, ago(a.created_ms));

    let el: HTMLElement;
    if (a.kind === 'sanction') {
      const sid = sanctionIdOf(a);
      const lift = sid !== null && suspended.has(sid) ? choice('Lift', { approve: false, action: 'lift', done: 'Lifted' }) : null;
      const acknowledge = choice('Acknowledge', { approve: true, action: 'acknowledge', done: 'Acknowledged', kind: 'primary' });
      const retire = choice('Retire', { approve: false, action: 'retire', done: 'Recorded', kind: 'danger' });
      el = card(null, [
        h('header.approval-head', null, who, h('strong', null, `Escalation: ${a.subject}`), age),
        h('div.approval-decide', null, reason, h('div.approval-buttons', null, retire, lift, acknowledge)),
        h('p.faint', null, 'Retire only records your decision. apid does not retire the agent.'),
      ]);
    } else if (a.kind === 'payment') {
      const deny = choice('Deny', { approve: false, done: 'Denied', kind: 'danger' });
      const approve = choice('Approve', { approve: true, done: 'Approved', kind: 'primary' });
      el = card(null, [
        h('header.approval-head', null, who, h('strong', null, `Payment awaiting approval: ${a.subject}`), age),
        h('pre.approval-input', null, JSON.stringify(a.detail, null, 2) ?? 'null'),
        h('div.approval-decide', null, reason, h('div.approval-buttons', null, deny, approve)),
      ]);
    } else {
      const no = choice('Deny', { approve: false, done: 'Denied', kind: 'danger' });
      const yes = choice('Approve', { approve: true, done: 'Approved', kind: 'primary' });
      el = card(null, [
        h('header.approval-head', null, who, h('code.approval-tool', null, a.tool), age),
        h('pre.approval-input', null, JSON.stringify(a.input, null, 2) ?? 'null'),
        h('div.approval-decide', null, reason, h('div.approval-buttons', null, no, yes)),
      ]);
    }
    el.classList.add('approval-card');
    el.setAttribute('data-key', `ap-${a.id}`);
    return el;
  };

  // One decided approval, as a single line of the history.
  const row = (a: Approval): HTMLElement =>
    h(
      'div.approval-row',
      { 'data-key': `ap-${a.id}` },
      badge(LABEL[a.state], TONE[a.state]),
      h('code.approval-row-tool', null, a.subject || a.tool),
      h('span.approval-row-agent', null, nameOf(a.cid)),
      a.reason ? h('span.approval-row-reason', null, a.reason) : null,
      h('time.approval-row-age', null, ago(a.created_ms)),
    );

  // One proposed agent, waiting: what it is for, why the orchestrator wants it, and the owner's call.
  const proposalCard = (p: AgentProposal): HTMLElement => {
    const title = p.role.title || p.role.id;
    const review = button('Review…', () => openAgentWizard({ proposal: p, prefill: p.role, onDone: () => void loadProposals() }), { kind: 'primary' });
    const reject = button('Reject', () => void rejectProposal(p), { kind: 'danger' });
    const el = card(
      null,
      h(
        'div.stack',
        null,
        h(
          'header.approval-head',
          null,
          h('strong', null, title),
          p.new_role ? badge('new role', 'accent') : null,
          badge(`${p.count} ${p.count === 1 ? 'agent' : 'agents'}`, 'info'),
          h('time.approval-age', null, ago(p.created_ms)),
        ),
        h('p', null, p.reason),
        p.instruction ? h('blockquote.proposal-instruction', null, `“${p.instruction}”`) : null,
        h('p.faint', null, `Proposed by ${p.by}`),
        h('div.approval-buttons', null, reject, review),
      ),
    );
    el.classList.add('approval-card');
    el.setAttribute('data-key', `pp-${p.id}`);
    return el;
  };

  // One decided proposal, as a line of the collapsed history.
  const proposalRow = (p: AgentProposal): HTMLElement =>
    h(
      'div.approval-row',
      { 'data-key': `pp-${p.id}` },
      badge(PROPOSAL_LABEL[p.state], PROPOSAL_TONE[p.state]),
      h('strong', null, p.role.title || p.role.id),
      p.note ? h('span.approval-row-reason', null, p.note) : null,
      h('time.approval-row-age', null, ago(p.decided_ms || p.created_ms)),
    );

  const renderProposals = (): void => {
    proposals.style.display = agentProposals.length ? '' : 'none';
    const waiting = agentProposals.filter((p) => p.state === 'pending').sort((x, y) => x.created_ms - y.created_ms);
    const past = agentProposals.filter((p) => p.state !== 'pending').sort((x, y) => y.created_ms - x.created_ms);
    replace(
      proposals,
      h('h2.approvals-subhead', null, 'New agents proposed by the orchestrator'),
      waiting.length ? waiting.map((p) => proposalCard(p)) : h('p.approvals-none', null, 'No proposals waiting.'),
      past.length ? h('details.proposals-decided', null, h('summary', null, `Decided (${past.length})`), past.map((p) => proposalRow(p))) : null,
    );
  };

  // A rejection needs a reason, as the wizard requires: a blank answer asks the same question again,
  // and Cancel gives up without sending anything.
  const rejectProposal = async (p: AgentProposal): Promise<void> => {
    const question = `Reject ${p.role.title || p.role.id}? Give a reason.`;
    let reason = window.prompt(question);
    while (reason !== null && reason.trim() === '') reason = window.prompt(question);
    if (reason === null) return;
    const note = reason.trim();
    await attempt(() => api.rejectAgentProposal(p.id, note), 'Rejected');
    void loadProposals();
  };

  const loadProposals = async (): Promise<void> => {
    try {
      agentProposals = await api.agentProposals();
    } catch {
      agentProposals = [];
    }
    if (alive) renderProposals();
  };

  const load = async (): Promise<void> => {
    try {
      items = await api.approvals();
      failed = false;
    } catch {
      failed = true;
    }
    // A failed register read hides every Lift until the next load, which is the safe side.
    const register = await api.sanctions().catch(() => null);
    suspended = new Set(register?.suspensions.filter((s) => s.active).map((s) => s.id) ?? []);
    loaded = true;
    if (alive) render();
  };

  const refetch = (): void => {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => void load(), DEBOUNCE_MS);
  };

  const offs: (() => void)[] = [on('approval', refetch), on('agent_proposal', () => void loadProposals()), agents.watch(() => render())];
  void load();
  void loadProposals();

  return () => {
    alive = false;
    window.clearTimeout(timer);
    offs.forEach((off) => off());
  };
}
