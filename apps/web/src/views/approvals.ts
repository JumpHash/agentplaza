// The approvals inbox, in the prototype's layout: the waiting list on the left (the orchestrator's proposed agents
// first, then the calls agents have asked to make), the chosen one's card on the right, and the decided ones folded
// under the list. Refetches on each `approval` frame.

import '../styles/proto/overview.css';
import { inScope, projectTitle, scope } from '../scope';
import { api, type AgentProposal, type Approval } from '../api';
import { review, type ReviewReading } from '../api-review';
import { href } from '../router';
import { agentLabelIn } from '../roles';
import { agents, on } from '../store';
import { ago, attempt, badge, button, dot, empty, modal, textarea, type Tone } from '../ui/components';
import { embedContextFromStore } from '../ui/embeds';
import { h, lu, replace } from '../ui/dom';
import type { LucideName } from '../ui/icons.gen';
import { renderMarkdown } from '../ui/markdown';
import type { Cleanup } from './index';
import { reviewPanel } from './review-graph';
import { readingTone } from './review-model';
import { waitTitle } from './overview';

const DEBOUNCE_MS = 300;

/** A review card's detail: the run it is about, and the summary the hub wrote for the owner. */
interface ReviewDetail {
  task: string;
  run: string;
  goal?: string;
  worker?: string;
  verdict?: string;
  needs_you?: string;
  readings?: ReviewReading[];
  change?: string;
  round?: number;
}

/** The review a `review` approval is about, when its detail names a run; any other approval has none. */
function reviewOf(a: Approval): ReviewDetail | null {
  const d = a.detail as Partial<ReviewDetail> | null;
  return a.kind === 'review' && d && typeof d.run === 'string' && d.run !== '' && typeof d.task === 'string' ? (d as ReviewDetail) : null;
}

/** A reading's dot on a review card: better is ok, worse is danger, neutral or not measured is muted. */
function readingDot(level: number | null): Tone {
  const t = readingTone(level);
  return t === 'better' ? 'ok' : t === 'worse' ? 'danger' : 'muted';
}

/** The share request an `artifact.share` approval is about: the artifact's title, the reason, and the options asked for. */
function shareOptions(a: Approval): { title: string; reason: string; options: string[] } {
  const d = a.detail as { artifact?: { title?: unknown }; options?: { expires_in_days?: unknown; allow_download?: unknown; version?: unknown }; reason?: unknown } | null;
  const artifactTitle = d?.artifact?.title;
  const days = d?.options?.expires_in_days;
  const version = d?.options?.version;
  return {
    title: typeof artifactTitle === 'string' && artifactTitle !== '' ? artifactTitle : a.subject,
    reason: typeof d?.reason === 'string' ? d.reason : '',
    options: [
      typeof days === 'number' && days ? `Expires in ${days} ${days === 1 ? 'day' : 'days'}` : 'Never expires',
      d?.options?.allow_download === true ? 'Downloading allowed' : 'View only',
      typeof version === 'number' && version ? `Pinned to v${version}` : 'Follows the latest version',
    ],
  };
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
const PROPOSAL_TONE: Record<AgentProposal['state'], 'ok' | 'danger' | 'warn'> = {
  pending: 'warn',
  approved: 'ok',
  rejected: 'danger',
};
const KIND_ICON: Record<string, LucideName> = { review: 'git-pull-request', tool: 'terminal', deploy: 'rocket' };

type Item = Approval | AgentProposal;
const isProposal = (x: Item): x is AgentProposal => 'role' in x;

export function mount(root: HTMLElement, params: string[]): Cleanup {
  // Under a project, the approvals its agents asked for (and the orchestrator's, which works on every project).
  const inProject = scope.get();
  const inbox = h('div.inbox');
  root.append(inbox);

  // The item on the right: the route's, or the first waiting one until the owner picks another.
  let selected: string | null = params[0] ?? null;
  let items: Approval[] = [];
  let loaded = false;
  let failed = false;
  let agentProposals: AgentProposal[] = [];
  let alive = true;
  let timer: number | undefined;
  // Reasons typed but not yet sent survive a re-render, keyed by approval id.
  const drafts = new Map<string, string>();
  // The review cards whose review is open under them, so a redraw keeps it open.
  const openReviews = new Set<string>();

  const nameOf = (cid: number): string => agentLabelIn(agents.get(), cid);
  const agentLink = (cid: number): HTMLElement => h('a.ap-agent', { href: href('sessions', cid) }, nameOf(cid));

  const shownHere = (a: Approval): boolean => {
    if (!inProject || a.cid === 0) return true;
    const agent = agents.get().find((x) => x.cid === a.cid);
    return !agent || inScope(agent, inProject);
  };

  const render = (): void => {
    if (!loaded) return;
    const shown = items.filter(shownHere);
    const waiting = shown.filter((a) => a.state === 'pending').sort((x, y) => x.created_ms - y.created_ms);
    const decided = shown.filter((a) => a.state !== 'pending').sort((x, y) => y.created_ms - x.created_ms);
    const pendingP = agentProposals.filter((p) => p.state === 'pending').sort((x, y) => x.created_ms - y.created_ms);
    const decidedP = agentProposals.filter((p) => p.state !== 'pending').sort((x, y) => (y.decided_ms || y.created_ms) - (x.decided_ms || x.created_ms));
    const open: Item[] = [...pendingP, ...waiting];
    const sel: Item | null = open.find((x) => x.id === selected) ?? open[0] ?? null;

    const list: (HTMLElement | null)[] = [];
    if (pendingP.length || agentProposals.length) {
      list.push(h('div.ibx-sec', null, 'New agents proposed by the orchestrator'));
      if (pendingP.length) list.push(...pendingP.map((p) => proposalItem(p, sel?.id === p.id)));
      else list.push(h('p.muted.ap-none', null, 'No proposals waiting.'));
      list.push(h('div.ibx-sec', null, 'Waiting'));
    }
    if (failed) list.push(h('div.ap-none', { role: 'alert' }, 'Could not load approvals.'));
    else list.push(...waiting.map((a) => approvalItem(a, sel?.id === a.id)));
    if (decidedP.length) list.push(h('details.ibx-dec', null, h('summary', null, lu('chevron-right', 13), `Decided proposals (${decidedP.length})`), ...decidedP.map((p) => proposalRow(p))));
    list.push(
      h(
        'details.ibx-dec',
        null,
        h('summary', null, lu('chevron-right', 13), `Decided (${decided.length})`),
        decided.length ? decided.map((a) => row(a)) : h('p.muted.ap-none', null, 'No decisions yet.'),
      ),
    );

    const detail = sel
      ? h('div.col', null, isProposal(sel) ? proposalCard(sel) : waitingCard(sel))
      : h('div.inbox-empty', null, empty('approval', 'Nothing is waiting', 'Agents ask here before acts that need your approval, and the orchestrator proposes new agents here.'));
    replace(inbox, h('div.inbox-list', { 'aria-label': 'Waiting' }, list), h('div.inbox-detail', null, detail));
  };

  // One line of the list: the kind's icon, the title, and who asked and when, opening it on the right.
  const item = (id: string, cur: boolean, icon: LucideName, kind: string, title: string, who: string, pj: string | null, created: number): HTMLElement => {
    const a = h(
      'a.ibx',
      { href: href('approvals', id), title, 'aria-current': cur ? 'page' : null },
      h('span.ii', { class: `k-${kind}` }, lu(icon, 16)),
      h(
        'span',
        { style: 'min-width:0' },
        h('b', null, title),
        h('small', null, pj ? h('span.ibx-pj', null, pj) : null, pj ? '·' : null, h('span', null, who), '·', h('span', null, ago(created))),
      ),
    );
    a.addEventListener('click', (e) => {
      e.preventDefault();
      selected = id;
      history.replaceState(null, '', href('approvals', id));
      render();
    });
    return a;
  };
  const approvalItem = (a: Approval, cur: boolean): HTMLElement => {
    const pid = agents.get().find((x) => x.cid === a.cid)?.project ?? null;
    return item(a.id, cur, KIND_ICON[a.kind] ?? 'shield-check', a.kind, waitTitle(a), nameOf(a.cid), pid ? projectTitle(pid) : null, a.created_ms);
  };
  const proposalItem = (p: AgentProposal, cur: boolean): HTMLElement =>
    item(p.id, cur, 'user-plus', 'proposal', p.role.title || p.role.id, p.by, null, p.created_ms);

  // The card's head: the kind's tile, the title, its chips, and the age on the right. Every card uses it.
  const detailHead = (created: number, icon: LucideName, kind: string, title: string, chips: Node[]): HTMLElement =>
    h(
      'header.ibd-head',
      null,
      h('span.ibd-ico', { class: `k-${kind}` }, lu(icon, 18)),
      h('div.ibd-titles', null, h('h3.ap-title.ibd-title', null, title), chips.length ? h('div.ibd-chips', null, chips) : null),
      h('span.ap-age.muted', null, ago(created)),
    );

  // The reason box of a card. The text typed survives a redraw.
  const reasonBox = (a: Approval): HTMLTextAreaElement => {
    const t = textarea({ rows: 2, placeholder: 'Reason (optional)', 'aria-label': 'Reason' });
    t.classList.add('ap-reason');
    t.value = drafts.get(a.id) ?? '';
    t.addEventListener('input', () => drafts.set(a.id, t.value));
    return t;
  };

  // A ruling: its button runs `act`, and every button of the card is disabled while it runs.
  const ruling = (group: HTMLButtonElement[], a: Approval, label: string, kind: 'primary' | 'danger' | undefined, act: () => Promise<unknown>, done: string): HTMLButtonElement => {
    const b = button(label, () => void settle(), { kind });
    group.push(b);
    const settle = async (): Promise<void> => {
      for (const x of group) x.disabled = true;
      const ok = await attempt(act, done);
      if (ok !== undefined) drafts.delete(a.id);
      void load();
    };
    return b;
  };

  // The review: the owner's call on one review round, with the review opened under the card.
  const reviewCard = (a: Approval, rv: ReviewDetail): HTMLElement => {
    const group: HTMLButtonElement[] = [];
    const reason = reasonBox(a);
    const isOpen = openReviews.has(a.id);
    const chips = (rv.readings ?? []).map((r) => h('span.ap-chip', null, dot(readingDot(r.level)), r.name));
    const worker = rv.worker?.trim() ? rv.worker : nameOf(a.cid);
    const meta = [`Worker: ${worker}`, rv.change ? `change ${rv.change}` : null, rv.round ? `round ${rv.round}` : null].filter(Boolean).join(' · ');
    const see = button(isOpen ? 'Hide the review' : 'See the review', () => {
      if (openReviews.has(a.id)) openReviews.delete(a.id);
      else openReviews.add(a.id);
      render();
    }, { size: 'sm' });
    const fail = ruling(group, a, 'Fail', 'danger', () => review.decide(a.id, 'fail', reason.value), 'Failed');
    const sendBack = ruling(group, a, 'Send back', undefined, () => review.decide(a.id, 'rework', reason.value), 'Sent back');
    const pass = ruling(group, a, 'Pass', 'primary', () => review.decide(a.id, 'pass', reason.value), 'Passed');
    return h(
      'article.card.ap-card.ibd.ap-review',
      { 'data-key': a.id },
      detailHead(a.created_ms, 'git-pull-request', 'review', rv.goal || `Review of ${rv.task}`, [agentLink(a.cid), badge('Review', 'accent')]),
      h(
        'div.ibd-body',
        null,
        rv.verdict ? h('p.review-verdict.ap-verdict', null, rv.verdict) : null,
        rv.needs_you ? h('p.ap-needs', null, h('strong', null, 'Needs you: '), rv.needs_you) : null,
        chips.length ? h('div.ap-chips', null, chips) : null,
        h('p.ap-meta', null, meta),
        h('div.ap-actions', null, see),
        isOpen ? h('div.ap-panel', null, reviewPanel(rv.task, rv.run)) : null,
      ),
      h('footer.ibd-foot', null, reason, h('div.ap-actions', null, fail, sendBack, pass)),
    );
  };

  // A tool call or a share: Deny and Approve, with what the agent asked for above them.
  const plainCard = (a: Approval, chips: Node[], body: Node): HTMLElement => {
    const group: HTMLButtonElement[] = [];
    const reason = reasonBox(a);
    const deny = ruling(group, a, 'Deny', 'danger', () => api.decideApproval(a.id, false, reason.value.trim() || undefined), 'Denied');
    const approve = ruling(group, a, 'Approve', 'primary', () => api.decideApproval(a.id, true, reason.value.trim() || undefined), 'Approved');
    return h(
      'article.card.ap-card.ibd',
      { 'data-key': a.id },
      detailHead(a.created_ms, KIND_ICON[a.kind] ?? 'shield-check', a.kind, waitTitle(a), chips),
      h('div.ibd-body', null, body),
      h('footer.ibd-foot', null, reason, h('div.ap-actions', null, deny, approve)),
    );
  };

  const waitingCard = (a: Approval): HTMLElement => {
    const rv = reviewOf(a);
    if (rv) return reviewCard(a, rv);
    if (a.kind === 'artifact.share') {
      const ask = shareOptions(a);
      return plainCard(
        a,
        [agentLink(a.cid), badge('Share', 'accent')],
        h('div.stack', null, ask.reason ? h('p.ap-md', null, ask.reason) : null, h('div.ap-chips', null, ask.options.map((o) => h('span.ap-chip', null, o)))),
      );
    }
    return plainCard(a, [agentLink(a.cid), h('code.ap-tool', null, a.tool)], h('pre.ap-pre', null, JSON.stringify(a.input, null, 2) ?? 'null'));
  };

  // A decided approval, as one line of the history.
  const row = (a: Approval): HTMLElement =>
    h(
      'div.ap-row',
      { 'data-key': a.id },
      badge(LABEL[a.state], TONE[a.state]),
      h('div.ap-row-main', null, h('code.ap-row-tool', null, a.subject || a.tool), h('span', null, agentLink(a.cid)), a.reason ? h('span.muted', null, a.reason) : null),
      h('span.muted.ap-age', null, ago(a.created_ms)),
    );

  // One proposed agent, waiting: what it is for, why the orchestrator wants it, and the owner's call.
  const proposalCard = (p: AgentProposal): HTMLElement => {
    const title = p.role.title || p.role.id;
    const badges = [p.new_role ? badge('new role', 'accent') : null, badge(p.count === 1 ? '1 agent' : `${p.count} agents`, 'info')].filter(
      (x): x is HTMLElement => x !== null,
    );
    const reject = button('Reject', () => void rejectProposal(p), { kind: 'danger' });
    const approve = button('Approve', () => approveProposal(p), { kind: 'primary' });
    return h(
      'article.card.ap-card.ibd',
      { 'data-key': `pp-${p.id}` },
      detailHead(p.created_ms, 'user-plus', 'proposal', title, badges),
      h(
        'div.ibd-body',
        null,
        // The orchestrator's rationale is agent-written text: rich content applies.
        h('div.ap-md', null, renderMarkdown(p.reason, { embeds: embedContextFromStore() })),
        p.instruction ? h('blockquote.ap-quote', null, `“${p.instruction}”`) : null,
        h('p.ap-by', null, `Proposed by ${p.by}`),
      ),
      h('footer.ibd-foot', null, h('div.ap-actions', null, reject, approve)),
    );
  };

  // One decided proposal, as a line of the collapsed history.
  const proposalRow = (p: AgentProposal): HTMLElement => {
    const label = p.state === 'approved' ? 'Approved' : 'Rejected';
    return h(
      'div.ap-row',
      { 'data-key': `pp-${p.id}` },
      badge(label, PROPOSAL_TONE[p.state]),
      h('div.ap-row-main', null, h('strong', null, p.role.title || p.role.id), p.note ? h('span.muted', null, p.note) : null),
      h('span.muted.ap-age', null, ago(p.decided_ms || p.created_ms)),
    );
  };

  /** Approves the orchestrator's proposal as it was written (the hub spawns the agents it asks for). */
  const approveProposal = (p: AgentProposal): void => {
    void attempt(() => api.approveAgentProposal(p.id), 'Approved').then(() => void loadProposals());
  };

  // A rejection needs a reason: a blank answer keeps the dialog open, and Cancel gives up.
  const rejectProposal = (p: AgentProposal): void => {
    const title = p.role.title || p.role.id;
    const ta = textarea({ rows: 3, 'aria-label': 'Reason' });
    ta.classList.add('ap-reason');
    let close: () => void = () => undefined;
    const confirm = button('Reject', () => {
      const note = ta.value.trim();
      if (!note) {
        ta.focus();
        return;
      }
      close();
      void attempt(() => api.rejectAgentProposal(p.id, note), 'Rejected').then(() => void loadProposals());
    }, { kind: 'danger' });
    const cancel = button('Cancel', () => close());
    close = modal(`Reject ${title}? Give a reason.`, h('div.stack', null, ta, h('div.ap-actions', null, cancel, confirm)));
  };

  const loadProposals = async (): Promise<void> => {
    try {
      agentProposals = await api.agentProposals();
    } catch {
      agentProposals = [];
    }
    if (alive) render();
  };

  const load = async (): Promise<void> => {
    try {
      items = await api.approvals();
      failed = false;
    } catch {
      failed = true;
    }
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
