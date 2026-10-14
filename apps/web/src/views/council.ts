import { roleLabel } from '../roles';
// The council: motions the plaza's roles vote on, and the owner's decisions on them.

import '../styles/ops.css';
import { api, type Motion } from '../api';
import { on } from '../store';
import { ago, attempt, badge, button, card, empty, modal, select, textarea, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

/** The states the hub reports (`State::as_str`), in the order a motion moves through them. */
const STATES = ['open', 'no_quorum', 'carried', 'rejected', 'tie_to_owner', 'awaiting_owner', 'ratified', 'vetoed'];

/** A motion waits for the owner's decision. */
const needsOwner = (m: Motion): boolean => m.state === 'tie_to_owner' || m.state === 'awaiting_owner';

/** The badge tone of each state, explicit so an unknown state reads as muted. */
const STATE_TONE: Record<string, Tone> = {
  open: 'info',
  no_quorum: 'muted',
  carried: 'ok',
  rejected: 'danger',
  tie_to_owner: 'warn',
  awaiting_owner: 'warn',
  ratified: 'ok',
  vetoed: 'danger',
};

const stateTone = (m: Motion): Tone => STATE_TONE[m.state] ?? 'muted';

const VOTE_TONE: Record<string, Tone> = { yes: 'ok', no: 'danger', abstain: 'muted' };

const share = (n: number, of: number): string => `${of > 0 ? (n / of) * 100 : 0}%`;

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  const list = h('div.stack');
  const stateSel = select([{ value: '', label: 'All states' }, ...STATES.map((s) => ({ value: s, label: s.replace(/_/g, ' ') }))]);
  root.append(h('div.view-head', null, h('h1', null, 'Council'), h('div.actions', null, stateSel)), list);

  let alive = true;
  let motions: Motion[] = [];
  const render = (): void => {
    if (!motions.length) {
      replace(list, empty('council', 'No motions', 'Council seats propose and vote on changes to how the plaza runs.'));
      return;
    }
    const shown = stateSel.value ? motions.filter((m) => m.state === stateSel.value) : motions;
    replace(list, shown.length ? shown.map((m) => motionCard(m, () => void refresh())) : empty('council', 'No motions in this state', 'Pick another state to see the rest.'));
  };
  stateSel.addEventListener('change', render);

  const refresh = async (): Promise<void> => {
    try {
      const loaded = await api.motions();
      if (!alive) return;
      motions = loaded;
      render();
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
        m.votes.map((v) => h('li', { 'data-key': `v-${v.seat}` }, h('span.muted', null, roleLabel(v.seat)), badge(v.vote, VOTE_TONE[v.vote] ?? 'muted'), h('span', null, v.reason))),
      )
    : null;
  const actions = needsOwner(m)
    ? h(
        'div.motion-actions',
        null,
        button('Reject', () => decide(m, false, changed), { kind: 'danger', icon: 'x' }),
        button('Approve', () => decide(m, true, changed), { kind: 'primary', icon: 'check' }),
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
      h('div.tally-legend.faint', null, `Quorum: ${m.quorum.rule}`),
      votes,
      actions,
    ),
  );
  el.setAttribute('data-key', `m-${m.id}`);
  return el;
}

function decide(m: Motion, approve: boolean, changed: () => void): void {
  const verb = approve ? 'Approve' : 'Reject';
  const reason = textarea({ rows: 3, placeholder: 'Reason' });
  const submit = button(
    verb,
    () => {
      submit.disabled = true;
      void attempt(() => api.decide(m.id, approve, reason.value.trim()), `Motion ${approve ? 'approved' : 'rejected'}`).then((done) => {
        if (done) {
          close();
          changed();
        } else {
          submit.disabled = false;
        }
      });
    },
    { kind: approve ? 'primary' : 'danger' },
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
