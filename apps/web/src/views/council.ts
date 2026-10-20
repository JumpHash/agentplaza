import { roleLabel } from '../roles';
// The council, as the Loom console draws it (the cn-* rules in styles/proto/social.css): the motions
// the plaza's roles vote on, each with its tally and its votes, and the owner's decisions on them.

import '../styles/proto/social.css';
import { api, type Motion } from '../api';
import { on } from '../store';
import { ago, attempt, button, empty, field, modal, select, textarea, type Tone } from '../ui/components';
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

const pct = (n: number, of: number): string => (of ? ((n / of) * 100).toFixed(2) : '0');

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  const list = h('div.cn-list');
  const stateSel = select([{ value: '', label: 'All states' }, ...STATES.map((s) => ({ value: s, label: s.replace(/_/g, ' ') }))]);
  stateSel.setAttribute('aria-label', 'State');
  stateSel.setAttribute('data-input', 'cn-filter');
  root.append(
    h(
      'header.page-head',
      null,
      h('div.page-head-row', null, h('h1', null, 'Council'), h('span.sp'), h('div.page-actions', null, h('div.cn-bar', null, stateSel))),
    ),
    list,
  );

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
    'div.cn-tally',
    { role: 'img', 'aria-label': `${t.yes} yes, ${t.no} no, ${t.abstain} abstain of ${t.seats}` },
    h('i.y', { style: `width:${pct(t.yes, t.seats)}%` }),
    h('i.n', { style: `width:${pct(t.no, t.seats)}%` }),
    h('i.a', { style: `width:${pct(t.abstain, t.seats)}%` }),
  );
  const votes = m.votes?.length
    ? h(
        'div.cn-votes',
        null,
        m.votes.map((v) =>
          h(
            'div.cn-vote',
            { 'data-key': `v-${v.seat}` },
            h('span.muted', null, roleLabel(v.seat)),
            h('span.chip', { class: `t-${VOTE_TONE[v.vote] ?? 'muted'}` }, v.vote),
            h('span', null, v.reason ?? ''),
          ),
        ),
      )
    : null;
  const actions = needsOwner(m)
    ? h(
        'div.cn-act',
        null,
        button('Reject', () => decide(m, false, changed), { kind: 'danger', size: 'sm', icon: 'x' }),
        button('Approve', () => decide(m, true, changed), { kind: 'primary', size: 'sm', icon: 'check' }),
      )
    : null;
  const el = h(
    'article.cn-card',
    { 'data-key': `m-${m.id}` },
    h('div.bd-top', null, h('span.chip.t-accent', null, m.kind), h('span.chip', { class: `t-${stateTone(m)}` }, m.state), h('span.bd-time', null, ago(m.created_at))),
    h('p.cn-text', null, m.text),
    bar,
    h('p.cn-leg', null, `${t.yes} yes · ${t.no} no · ${t.abstain} abstain · ${t.voted}/${t.seats}`),
    h('p.hint', null, `Quorum: ${m.quorum.rule}`),
    votes,
    actions,
  );
  return el;
}

function decide(m: Motion, approve: boolean, changed: () => void): void {
  const verb = approve ? 'Approve' : 'Reject';
  const reason = textarea({ id: 'cn-reason', rows: 3, placeholder: 'Reason', 'aria-label': 'Reason' });
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
      'div',
      null,
      h('p.muted', null, m.text),
      field('Reason', reason),
      h('footer.sheet-f', null, button('Cancel', () => close(), { kind: 'ghost' }), submit),
    ),
  );
}
