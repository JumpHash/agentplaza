// The History screen's two drawings, as the prototype draws them (the rails, the nodes and the step dots are CSS): the
// lane graph, which is the line of main with its landed changes as nodes and the open changes as branches off it; and
// a change's sign-off chain, its six steps as a row of dots that take the outcome's colour.

import type { WeftChangeSummary, WeftHistory } from '../api-weft';
import { href } from '../router';
import { ago, badge, clip } from '../ui/components';
import { h, lu } from '../ui/dom';
import { changeStateLabel, changeStateTone, intentChips, layoutHistory, plural, type ChainDot, type ChainRound, type ChainStepView } from './history-model';

/** The actor's name for a label such as `agent:301`, supplied by the view (the roster knows the names). */
export type Namer = (label: string) => string;

const TONE_OF_STATE: Record<string, string> = { open: 'muted', sealed: 'info', queued: 'info', conflict: 'danger', landed: 'ok', abandoned: 'muted' };

/** The lane graph: the open lanes on top as branches off the head of main, then main's landed changes, newest first. */
export function laneGraph(repo: string, main: WeftChangeSummary[], lanes: WeftChangeSummary[], head: string, name: Namer): HTMLElement {
  const rows: Node[] = [
    ...lanes.map((c) => laneRow(repo, c, name)),
    h(
      'li',
      { class: 'hx-row hx-head' },
      h('span.hx-rail', { 'aria-hidden': 'true' }, h('i.hx-node')),
      h(
        'span',
        null,
        lu('git-branch', 15),
        'main ',
        h('code', null, head),
        h('span.hx-faint', null, `${plural(lanes.length, 'open lane')} above, ${plural(main.length, 'landed change')} below`),
      ),
    ),
    ...main.map((c) => laneRow(repo, c, name, true)),
  ];
  return h('ol.hx-graph', { 'aria-label': 'Changes on main and in lanes' }, ...rows);
}

/** The landings as a graph (21.6): main's commits, newest on top, each with its content id, its author, its time, the git
 *  commit each connection received and its records; the open lanes sit in columns of their own, beside the landing
 *  they start from. */
export function landingGraph(history: WeftHistory): HTMLElement {
  const { nodes } = layoutHistory(history);
  const landings = new Map(history.landings.map((l) => [l.id, l]));
  const lanes = new Map(history.lanes.map((l) => [l.change, l]));
  const rows = [...nodes].sort((a, b) => a.row - b.row || a.column - b.column);
  return h(
    'ol.hx-graph.hx-landings',
    { 'aria-label': 'Landings on main as commits, and the open lanes' },
    rows.map((n) => {
      const rail = h('span.hx-rail', { 'aria-hidden': 'true', style: `margin-left:${n.column * 16}px` }, h('i.hx-node'));
      if (n.kind === 'lane') {
        const lane = lanes.get(n.id);
        if (!lane) return null;
        return h(
          'li',
          { class: `hx-row hx-lane is-${lane.state} t-${TONE_OF_STATE[lane.state] ?? 'muted'}` },
          rail,
          h('a.hx-rowlink', { href: href('history', history.repo, 'change', lane.change) }, h('span.hx-id', null, lane.change), h('span.hx-title', null, lane.title), badge(changeStateLabel(lane.state), changeStateTone(lane.state))),
        );
      }
      const l = landings.get(n.id);
      if (!l) return null;
      const git = Object.values(l.exported)[0];
      const meta = [l.author?.name ?? '', l.imported ? 'imported' : '', l.time_ms ? ago(l.time_ms) : ''].filter(Boolean).join(' · ');
      return h(
        'li',
        { class: 'hx-row hx-main' },
        rail,
        h(
          'a.hx-rowlink',
          { href: href('history', history.repo, 'change', l.change) },
          h('span.hx-id', null, l.id),
          h('span.hx-title', null, l.title),
          h('span.hx-faint', null, meta),
          git ? h('code', null, git.slice(0, 10)) : h('span'),
          h('span.hx-faint', null, `${plural(l.records.length, 'record')}`),
        ),
      );
    }),
  );
}

function laneRow(repo: string, c: WeftChangeSummary, name: Namer, onMain = false): HTMLElement {
  // An imported change is by its git author (`Name <email>`), shown by name.
  const author = c.imported ? c.imported.author.replace(/\s*<[^>]*>$/, '') : c.author ? name(c.author) : '';
  const meta = [author, c.imported ? 'imported' : '', c.at_ms ? ago(c.at_ms) : ''].filter(Boolean).join(' · ');
  const intents = c.intents.length ? ` · ${plural(c.intents.length, 'intent')}` : '';
  const chips = intentChips(c);
  const more = c.intents.length > chips.length ? h('span.hx-chip', null, `+${c.intents.length - chips.length}`) : null;
  const tone = onMain ? 'ok' : (TONE_OF_STATE[c.state] ?? 'muted');
  return h(
    'li',
    { class: `hx-row ${onMain ? 'hx-main' : `hx-lane is-${c.state} t-${tone}`}` },
    h('span.hx-rail', { 'aria-hidden': 'true' }, h('i.hx-node')),
    h(
      'a.hx-rowlink',
      { href: href('history', repo, 'change', c.change) },
      h('span.hx-id', null, c.change),
      h('span.hx-title', null, c.title),
      onMain ? h('span') : badge(changeStateLabel(c.state), changeStateTone(c.state)),
      h(
        'span.hx-chips',
        null,
        chips.map((i) => h('span.hx-chip', null, h('b', null, i.kind), ` ${clip(i.title, 40)}`)),
        more,
      ),
      h('span.hx-meta', null, `${meta}${intents}`),
    ),
  );
}

/** A change's sign-off, one chain per round, the latest last. The current step of the last round takes the ring. */
export function signOffView(rounds: ChainRound[], name: Namer): HTMLElement {
  return h('div', null, ...rounds.map((r, i) => chainRow(r, i === rounds.length - 1, rounds.length, name)));
}

function chainRow(round: ChainRound, last: boolean, total: number, name: Namer): HTMLElement {
  const sentBack = round.steps.some((s) => s.step === 'gate' && s.outcome === 'warn');
  const landed = round.steps.some((s) => s.step === 'landed' && s.outcome === 'ok');
  const label = total === 1 ? 'Sign-off' : `Round ${round.round}`;
  const note = sentBack ? 'sent back' : landed ? 'landed' : last ? 'current' : '';
  // The current round's first step with no record yet is where the sign-off stands.
  const frontier = last && !landed ? round.steps.findIndex((s) => s.outcome === 'pending') : -1;
  return h(
    'div',
    null,
    h('div.hx-round', null, h('b', null, label), note ? h('span.hx-faint', null, note) : null),
    h(
      'ol.hx-steps',
      { 'aria-label': `${label} sign-off` },
      round.steps.map((s, i) => stepView(s, name, i === frontier)),
    ),
  );
}

function stepView(s: ChainStepView, name: Namer, current: boolean): HTMLElement {
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
  const tip = s.dots.length ? s.dots.map((d) => dotTip(d, name)).join('; ') : s.outcome === 'owner' ? 'Waiting for you' : 'Not yet';
  return h(
    'li',
    { class: `hx-step is-${s.outcome}${current ? ' is-current' : ''}`, 'data-step': s.step },
    h('span.hx-dot-s', { class: `is-${s.outcome}`, title: tip, 'aria-label': tip }),
    h('span.hx-step-n', null, s.label),
    h('span.hx-step-s', null, sub, fromConsole ? ' ' : null, fromConsole ? badge('from the console', 'accent') : null),
  );
}

function dotTip(d: ChainDot, name: Namer): string {
  return `${name(d.signer)} · ${ago(d.at_ms)}${d.text ? ` · ${d.text}` : ''}`;
}
