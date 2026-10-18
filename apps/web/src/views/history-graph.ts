// The History screen's two drawings, in plain DOM (the rails and dots are CSS): the lane graph, which is the line of
// main with its landed changes as dots and the open changes as side branches; and a change's sign-off chain, its steps
// as a row of dots that take the outcome's colour.

import type { WeftChangeSummary } from '../api-weft';
import { href } from '../router';
import { ago, badge, clip } from '../ui/components';
import { h } from '../ui/dom';
import { changeStateLabel, changeStateTone, intentChips, plural, type ChainDot, type ChainRound, type ChainStepView } from './history-model';

/** The actor's name for a label such as `agent:301`, supplied by the view (the roster knows the names). */
export type Namer = (label: string) => string;

/** The lane graph: the open lanes on top as side branches off the head of main, then main's landed changes, newest
 * first, each a dot on one line. */
export function laneGraph(repo: string, main: WeftChangeSummary[], lanes: WeftChangeSummary[], head: string, name: Namer): HTMLElement {
  if (!main.length && !lanes.length) {
    return h('p.hx-faint', null, 'No changes match this query.');
  }
  const rows: HTMLElement[] = [
    ...lanes.map((c) => laneRow(repo, c, 'lane', name)),
    h('li', { class: 'hx-row hx-head' }, h('div.hx-rail', null, h('span.hx-dot', { class: 'is-head' })), h('span.hx-main-label', null, 'main', h('code', null, head))),
    ...main.map((c) => laneRow(repo, c, 'main', name)),
  ];
  return h('ol.hx-graph', { 'aria-label': 'Changes on main and in lanes' }, rows);
}

function laneRow(repo: string, c: WeftChangeSummary, kind: 'lane' | 'main', name: Namer): HTMLElement {
  const tone = kind === 'main' ? 'ok' : changeStateTone(c.state);
  const who = [c.author ? name(c.author) : '', c.at_ms ? ago(c.at_ms) : ''].filter(Boolean).join(' · ');
  const chips = intentChips(c);
  return h(
    'li',
    { class: `hx-row hx-${kind} is-${c.state}` },
    h('div.hx-rail', null, h('span.hx-dot', { class: `is-${tone}`, 'aria-hidden': 'true' })),
    h(
      'a.hx-body',
      { href: href('history', repo, 'change', c.change) },
      h(
        'span.hx-line-top',
        null,
        h('span.hx-id', null, c.change),
        h('span.hx-title', null, c.title),
        kind === 'lane' ? badge(changeStateLabel(c.state), changeStateTone(c.state)) : null,
      ),
      chips.length
        ? h(
            'span.hx-chips',
            null,
            chips.map((i) => h('span.hx-chip', { title: i.title }, h('span.hx-chip-kind', null, i.kind), clip(i.title, 40))),
            c.intents.length > chips.length ? h('span.hx-chip-more', null, `+${c.intents.length - chips.length}`) : null,
          )
        : null,
      h('span.hx-meta', null, who || null, c.intents.length ? ` · ${plural(c.intents.length, 'intent')}` : null),
    ),
  );
}

/** A change's sign-off, one chain per round, the latest last. */
export function signOffView(rounds: ChainRound[], name: Namer): HTMLElement {
  if (!rounds.length) return h('p.hx-faint', null, 'Nothing has been signed yet.');
  return h(
    'div.hx-chains',
    null,
    rounds.map((r, i) => chainRow(r, i === rounds.length - 1, rounds.length, name)),
  );
}

function chainRow(round: ChainRound, last: boolean, total: number, name: Namer): HTMLElement {
  const sentBack = round.steps.some((s) => s.step === 'gate' && s.outcome === 'warn');
  const landed = round.steps.some((s) => s.step === 'landed' && s.outcome === 'ok');
  const word = total > 1 ? `Round ${round.round}` : 'Sign-off';
  const note = sentBack ? 'sent back' : landed ? 'landed' : last ? 'current' : '';
  return h(
    'div.hx-chain-wrap',
    null,
    h('div.hx-chain-label', null, word, note ? h('span.hx-chain-note', null, ` · ${note}`) : null),
    h(
      'ol.hx-chain',
      { 'aria-label': `${word} sign-off` },
      round.steps.map((s) => stepView(s, name)),
    ),
  );
}

function stepView(s: ChainStepView, name: Namer): HTMLElement {
  const dots: Node[] = s.dots.length
    ? s.dots.map((d) => dotView(d, name))
    : [h('span.hx-dot', { class: `is-${s.outcome}`, title: s.outcome === 'owner' ? 'Waiting for you' : 'Not yet', 'aria-label': s.label })];
  const first = s.dots[0];
  const sub =
    s.outcome === 'owner'
      ? s.step === 'ruling'
        ? 'waits for you'
        : 'sent to you'
      : s.step === 'review' && s.dots.length
        ? plural(s.dots.length, 'review')
        : first
          ? name(first.signer)
          : '';
  const fromConsole = s.step === 'ruling' && s.dots.some((d) => d.console);
  return h(
    'li',
    { class: `hx-step is-${s.outcome}`, 'data-step': s.step },
    h('div.hx-step-dots', null, dots),
    h('span.hx-step-label', null, s.label),
    h('span.hx-step-who', null, sub || null),
    fromConsole ? h('span.hx-console', null, 'from the console') : null,
  );
}

function dotView(d: ChainDot, name: Namer): HTMLElement {
  const tip = `${name(d.signer)} · ${ago(d.at_ms)}${d.text ? ` · ${d.text}` : ''}`;
  return h('span.hx-dot', {
    class: `is-${d.outcome}${d.console ? ' is-console' : ''}`,
    title: tip,
    role: 'img',
    'aria-label': tip,
  });
}
