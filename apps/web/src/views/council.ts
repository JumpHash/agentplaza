import { roleLabel } from '../roles';
// The council: motions the plaza's roles vote on, and the owner's decisions on them.

import '../styles/ops.css';
import { api, type Motion } from '../api';
import { on } from '../store';
import { ago, attempt, badge, button, card, empty, modal, textarea, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

const needsOwner = (m: Motion): boolean => /owner|await/i.test(m.state);

function stateTone(m: Motion): Tone {
  if (needsOwner(m)) return 'warn';
  if (/carr|pass|approv/i.test(m.state)) return 'ok';
  if (/reject|fail|den/i.test(m.state)) return 'danger';
  if (/open/i.test(m.state)) return 'info';
  return 'muted';
}

const VOTE_TONE: Record<string, Tone> = { yes: 'ok', no: 'danger', abstain: 'muted' };

const share = (n: number, of: number): string => `${of > 0 ? (n / of) * 100 : 0}%`;

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  const list = h('div.stack');
  root.append(h('div.view-head', null, h('h1', null, 'Council')), list);

  let alive = true;
  const refresh = async (): Promise<void> => {
    try {
      const motions = await api.motions();
      if (!alive) return;
      replace(list, motions.length ? motions.map((m) => motionCard(m, () => void refresh())) : empty('council', 'No motions', 'Council seats propose and vote on changes to how the plaza runs.'));
    } catch (err) {
      if (alive) replace(list, empty('alert', 'Council unreachable', err instanceof Error ? err.message : String(err)));
    }
  };

  const off = on('motion', () => void refresh());
  void refresh();
  return () => {
    alive = false;
    off();
  };
}

function motionCard(m: Motion, changed: () => void): HTMLElement {
  const t = m.tally;
  const bar = h(
    'div.tally',
    { role: 'img', 'aria-label': `${t.yes} yes, ${t.no} no, ${t.abstain} abstain of ${t.seats}` },
    h('span.tally-seg.yes', { style: `width:${share(t.yes, t.seats)}` }),
    h('span.tally-seg.no', { style: `width:${share(t.no, t.seats)}` }),
    h('span.tally-seg.abstain', { style: `width:${share(t.abstain, t.seats)}` }),
  );
  const votes = m.votes?.length
    ? h(
        'ul.votes',
        null,
        m.votes.map((v) => h('li', { 'data-key': `v-${v.role}` }, h('span.muted', null, roleLabel(v.role)), badge(v.vote, VOTE_TONE[v.vote] ?? 'muted'), h('span', null, v.reason))),
      )
    : null;
  const actions = needsOwner(m)
    ? h(
        'div.motion-actions',
        null,
        button('Reject', () => decide(m, 'reject', changed), { kind: 'danger', icon: 'x' }),
        button('Approve', () => decide(m, 'approve', changed), { kind: 'primary', icon: 'check' }),
      )
    : null;
  const el = card(
    h('div.motion-head', null, badge(m.kind, 'accent'), badge(m.state, stateTone(m), true), h('span.faint', null, ago(m.created_at))),
    h(
      'div.stack',
      null,
      h('p.motion-text', null, m.text),
      bar,
      h('div.tally-legend.faint', null, `${t.yes} yes · ${t.no} no · ${t.abstain} abstain · ${t.voted}/${t.seats}`),
      votes,
      actions,
    ),
  );
  el.setAttribute('data-key', `m-${m.id}`);
  return el;
}

function decide(m: Motion, decision: 'approve' | 'reject', changed: () => void): void {
  const verb = decision === 'approve' ? 'Approve' : 'Reject';
  const reason = textarea({ rows: 3, placeholder: 'Reason' });
  const submit = button(
    verb,
    () => {
      submit.disabled = true;
      void attempt(() => api.decide(m.id, decision, reason.value.trim()), `Motion ${decision === 'approve' ? 'approved' : 'rejected'}`).then((done) => {
        if (done) {
          close();
          changed();
        } else {
          submit.disabled = false;
        }
      });
    },
    { kind: decision === 'approve' ? 'primary' : 'danger' },
  );
  const close = modal(
    `${verb} motion`,
    h(
      'div.stack',
      null,
      h('p.muted', null, m.text),
      reason,
      h('div.motion-actions', null, button('Cancel', () => close(), { kind: 'ghost' }), submit),
    ),
  );
}
