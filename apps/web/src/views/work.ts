// Work: a project as one flow. Its planning (brief, research, proposal, critique, approval) while it plans; then its
// tasks in the order their `after` links let them run, drawn as a map with the plan on the left and delivery on the
// right; a project whose tasks have no links shows them in state columns. A card says who has the task and what they
// are doing now, or what it waits for. "Only what needs me" keeps the decisions and the work they hold up. Cards never
// move while a project's graph is unchanged. Tasks outside projects have a tab of their own.
// The markup is the prototype's (m-work.js, styles/proto/work.css); the layout is the wasm one the desktop draws.

import '../styles/proto/work.css';
import { scope } from '../scope';
import { api, PROJECT_STAGES, type AgentStep, type GraphTask, type Project, type ProjectGraph, type Task } from '../api';
import { loadGraph, type GraphLayout, type Layout } from '../graph/wasm';
import { agentLabel, agentName, roleLabel } from '../roles';
import { go, href } from '../router';
import { agents, on } from '../store';
import { attempt, badge, button, type Tone } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
import type { Cleanup } from './index';
import {
  agoLabel,
  cardLine,
  ended,
  FLOW_TASKS,
  flowSegments,
  looseGraph,
  needsMe,
  planCards,
  readySince,
  shortSpan,
  STATE_COLUMNS,
  STATE_LABEL,
  titleOf,
  upstreamOf,
  waitsForOwner,
  workState,
  type PlanCard,
  type Seg,
  type WorkState,
} from './work-model';

const DEBOUNCE_MS = 300;
/** The tab of the tasks outside projects. */
const LOOSE = 'outside';
/** The map's sizes, as the prototype draws them: task cards, the Plan column and delivery, and the gaps between. */
const CW = 232;
const CH = 132;
const GX = 48;
const GY = 14;
const PW = 220;
const PH = 72;
const PG = 14;
/** The map's padding above its first card and below its last. */
const TOP = 16;
const CARD = { card_width: CW, card_height: CH, gap_x: GX, gap_y: GY, pad: 12 };
const SVG = 'http://www.w3.org/2000/svg';
const LENS_KEY = 'apz.work.lens';
const SUB = 'A project as one flow: its plan (brief to approval), its tasks in the order their after links let them run, and delivery.';

/** A state's badge. Orange (accent) is the owner's hand: only what waits on the owner wears it. */
const TONE: Record<WorkState, Tone> = {
  ready: 'info',
  waiting: 'muted',
  blocked: 'danger',
  working: 'info',
  'needs-you': 'accent',
  review: 'info',
  done: 'ok',
  failed: 'danger',
  withdrawn: 'muted',
};

const cap = (s: string): string => (s ? s.charAt(0).toUpperCase() + s.slice(1) : '');

function readLens(): boolean {
  try {
    return localStorage.getItem(LENS_KEY) === '1';
  } catch {
    return false;
  }
}

function writeLens(on: boolean): void {
  try {
    localStorage.setItem(LENS_KEY, on ? '1' : '0');
  } catch {
    // A refused store keeps the toggle for this visit only.
  }
}

/** A flow line: the plan's squares, a square per task, then review and delivery. Used by the strip and the Overview. */
export function flowLine(segs: Seg[], more = 0): HTMLElement {
  return h(
    'span.wk-flow',
    { 'aria-hidden': 'true' },
    segs.map((s) => h('span.wk-sq', { class: squareClass(s), title: s.label })),
    more > 0 ? h('span.wk-note', null, `+${more}`) : null,
  );
}

/** A square's class: a plan stage or delivery by its place (done, current, you); a task by its state. */
function squareClass(s: Seg): string {
  if (s.kind !== 'task') return s.state === 'done' || s.state === 'current' || s.state === 'you' ? s.state : '';
  const st = s.state as WorkState;
  const kind = ended(st) ? 'ended' : st === 'needs-you' ? 'need' : st === 'working' || st === 'review' ? 'run' : '';
  return ['task', kind].filter(Boolean).join(' ');
}

function svgEl<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string>): SVGElementTagNameMap[K] {
  const el = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  return el;
}

/** The page head: the title, the actions at its right, and the lede under it. */
export function pageHead(title: string, sub: string | null, actions: Node | null = null): HTMLElement {
  return h(
    'header.page-head',
    null,
    h('div.page-head-row', null, h('h1', null, title), h('span.sp'), h('div.page-actions', null, actions)),
    sub ? h('p.lede', null, sub) : null,
  );
}

/** A checkbox in the prototype's check markup: a label with the box and its words. */
export function checkBox(checked: boolean, label: string, onChange: (on: boolean) => void): HTMLElement {
  const box = h('input', { type: 'checkbox', checked: checked || null }) as HTMLInputElement;
  box.addEventListener('change', () => onChange(box.checked));
  return h('label.check', null, box, h('span', null, label));
}

/** An empty state: the icon, a title, and the words under it. */
export function emptyBox(icon: Parameters<typeof lu>[0], title: string, text?: string, action?: Node): HTMLElement {
  return h('div.empty', null, lu(icon, 22), h('b', null, title), text ? h('span', null, text) : null, action ?? null);
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  // A tab named in the route, else the project the console is scoped to. The page's children are drawn into the view
  // directly (the view is a fresh element per mount), as the prototype's page lays them out.
  const wanted = params[0] ?? scope.get() ?? '';
  const screen = root;

  let alive = true;
  let loaded = false;
  let timer = 0;
  let projects: Project[] = [];
  let tasks: Task[] = [];
  let graph: ProjectGraph | null = null;
  let current = '';
  let selected = '';
  let lens = readLens();
  let layouter: GraphLayout | null = null;
  let layoutError = '';
  let layoutKey = '';
  let layout: Layout | null = null;
  let detail: Task | null = null;
  const steps = new Map<number, AgentStep | null>();

  const nameOf = (cid: number): string => {
    const a = agents.get().find((x) => x.cid === cid);
    return a ? agentName(a) : agentLabel(cid);
  };
  const stepOf = (cid: number): AgentStep | null | undefined =>
    steps.has(cid) ? steps.get(cid) : agents.get().find((a) => a.cid === cid)?.step;

  /** The tasks the open tab shows, with their links: a project's graph, or the tasks outside projects. */
  const shown = (): { tasks: GraphTask[]; edges: [string, string][] } =>
    current === LOOSE ? looseGraph(tasks) : graph ? { tasks: graph.tasks, edges: graph.edges } : { tasks: [], edges: [] };
  const projectOf = (id: string): Project | undefined => projects.find((p) => p.id === id);
  const tasksOf = (p: Project | null): Task[] =>
    p ? tasks.filter((t) => t.project === p.id || p.tasks.includes(t.id)) : tasks.filter((t) => !t.project);

  const stateOf = (t: GraphTask): WorkState => workState(t, t.agent_cid !== null ? stepOf(t.agent_cid) : null);
  /** When a task in the ledger's list became ready; a task the list does not have yet has no time. The index is
   *  rebuilt once per list the hub sends, not once per card. */
  let index: { of: Task[]; byId: Map<string, Task> } | null = null;
  const readyAt = (id: string): number | undefined => {
    if (index?.of !== tasks) index = { of: tasks, byId: new Map(tasks.map((t) => [t.id, t])) };
    const t = index.byId.get(id);
    return t ? readySince(t, index.byId) : undefined;
  };

  // ---- The strip: a tab per project, with its flow line ----

  /** A tab's second line: the stage (a project) and what it waits for, or its task count. */
  const subOf = (p: Project | null, own: Task[]): string => {
    const count = `${own.length} ${own.length === 1 ? 'task' : 'tasks'}`;
    const waits = waitsForOwner(p, own, stepOf);
    if (!p) return waits ? `${count} · waits for you` : count;
    return `${cap(p.stage)} · ${waits ? 'waits for you' : count}`;
  };

  function strip(): HTMLElement {
    const loose = tasks.filter((t) => !t.project);
    const tab = (key: string, title: string, sub: string, segs: Seg[], more: number): HTMLElement =>
      h(
        'a.wk-tab',
        { href: href('work', key), 'aria-current': key === current ? 'page' : null, 'data-key': `tab-${key}` },
        h('b', null, title),
        h('small', null, sub),
        flowLine(segs, more),
      );
    return h(
      'nav.wk-strip',
      { 'aria-label': 'Projects' },
      projects.map((p) => {
        const own = tasksOf(p);
        return tab(p.id, p.title, subOf(p, own), flowSegments(p, own, stepOf), Math.max(0, own.length - FLOW_TASKS));
      }),
      loose.length ? tab(LOOSE, 'Outside projects', subOf(null, loose), flowSegments(null, loose, stepOf), Math.max(0, loose.length - FLOW_TASKS)) : null,
    );
  }

  // ---- Cards ----

  /** What the cards of one drawing share: the links, and the hub's state of each task, built once, not per card. */
  interface Cards {
    edges: [string, string][];
    hubState: Map<string, string>;
  }
  const cardsOf = (g: { tasks: GraphTask[]; edges: [string, string][] }): Cards => ({ edges: g.edges, hubState: new Map(g.tasks.map((t) => [t.id, t.state])) });

  /** A task's card: its id and role, its title, its line (who has it, or what it waits for), and its state. */
  function taskCard(t: GraphTask, pos: { left: number; top: number } | null, faded: boolean, cards: Cards): HTMLElement {
    const state = stateOf(t);
    const line = cardLine(t, state, {
      edges: cards.edges,
      stateOf: (id) => cards.hubState.get(id),
      taskName: (id) => id,
      readyAt,
      nameOf,
      stepOf,
      now: Date.now(),
    });
    const cls = [ended(state) ? 'ended' : '', state === 'needs-you' ? 's-accent' : '', faded ? 'wk-faded' : ''].filter(Boolean).join(' ');
    const card = h(
      'button.wk-card',
      {
        type: 'button',
        class: cls,
        style: pos ? `left:${pos.left}px;top:${pos.top}px;width:${CW}px;height:${CH}px` : null,
        'data-id': t.id,
        'data-cid': t.agent_cid ?? '',
        'data-reviewer': t.reviewer_cid ?? '',
        'aria-pressed': t.id === selected ? 'true' : 'false',
      },
      h('span.wk-id', null, h('span', null, t.id), h('span', null, t.role ? roleLabel(t.role) : 'Any role')),
      h('span.wk-card-title', null, titleOf(t.goal)),
      h('span.wk-line', null, line),
      h('span', null, badge(STATE_LABEL[state], TONE[state], true)),
    );
    card.addEventListener('click', () => select(t.id));
    return card;
  }

  /** A card of the Plan column: a stage, its line, and the project it is in. */
  function planCard(c: PlanCard, p: Project, top: number): HTMLElement {
    return h(
      'a.wk-plan-card',
      { href: href('projects', p.id), class: c.state === 'you' ? 'you' : c.state === 'done' ? 'done' : '', title: `${c.title}: ${c.line}`, style: `top:${top}px;width:${PW}px;height:${PH}px` },
      h('span.wk-pt', null, c.title),
      h('span.wk-line', null, c.line),
    );
  }

  /** Delivery: the last box of the map, when the project's tasks are all in. */
  function deliveryCard(p: Project, box: { left: number; top: number }): HTMLElement {
    const delivered = p.stage === 'delivered';
    const dState = delivered ? 'done' : p.stage === 'review' ? 'current' : 'later';
    return h(
      'div.wk-delivery',
      { class: dState, style: `left:${box.left}px;top:${box.top}px;width:${PW}px;height:${PH}px` },
      h('span.wk-pt', null, delivered ? 'Delivered' : 'Review, then delivered'),
      h('span.wk-line', null, delivered ? 'The lead attached the deliverable' : 'Once every task is accepted, the lead attaches the deliverable'),
    );
  }

  // ---- The map ----

  /** The graph's layout, kept while its tasks and links are the same: cards never move under the reader. */
  function layoutOf(g: { tasks: GraphTask[]; edges: [string, string][] }): Layout | null {
    if (!layouter) return null;
    const key = g.tasks.map((t) => t.id).join(',') + '|' + g.edges.map((e) => e.join('>')).join(',');
    if (key !== layoutKey || !layout) {
      try {
        layout = layouter.layout(g, CARD);
        layoutKey = key;
      } catch (e) {
        layoutError = (e as Error).message;
        layout = null;
      }
    }
    return layout;
  }

  /** The map: the plan at the left, the tasks by depth with their links between, and delivery at the right. Each link is a
   *  curve from the card it leaves to the one it enters, with an arrowhead; the links from the plan and to delivery are dashed. */
  function mapOf(g: { tasks: GraphTask[]; edges: [string, string][] }, p: Project | null, kept: Set<string> | null, laid: Layout): HTMLElement {
    const plan = p ? planCards(p) : [];
    const planH = plan.length * (PH + PG);
    const ox = p ? PW + GX : 0;
    const pos = new Map(laid.cards.map((c) => [c.id, { x: ox + c.x, y: TOP + c.y }]));
    const dx = ox + laid.width + GX;
    const width = p ? dx + PW : laid.width;
    const height = Math.max(planH, TOP + laid.height) + TOP;
    const mid = height / 2;
    const states = new Map(g.tasks.map((t) => [t.id, stateOf(t)]));
    const bez = (x1: number, y1: number, x2: number, y2: number): string => `M${x1},${y1} C${(x1 + x2) / 2},${y1} ${(x1 + x2) / 2},${y2} ${x2},${y2}`;
    const svg = svgEl('svg', { width: String(width), height: String(height), 'aria-hidden': 'true' });
    const defs = svgEl('defs', {});
    const marker = svgEl('marker', { id: 'wk-arrow', viewBox: '0 0 10 10', refX: '9', refY: '5', markerWidth: '7', markerHeight: '7', orient: 'auto-start-reverse' });
    marker.append(svgEl('path', { d: 'M0,0 L10,5 L0,10 z', class: 'wk-arrow' }));
    defs.append(marker);
    svg.append(defs);
    for (const [a, b] of g.edges) {
      const A = pos.get(a), B = pos.get(b);
      if (!A || !B) continue;
      const to = states.get(b);
      svg.append(svgEl('path', { d: bez(A.x + CW, A.y + CH / 2, B.x, B.y + CH / 2), class: `wk-route${to !== undefined && ended(to) ? ' dim' : ''}`, 'marker-end': 'url(#wk-arrow)' }));
    }
    if (p) {
      for (const t of g.tasks) {
        const B = pos.get(t.id);
        if (B && t.depth === 0) svg.append(svgEl('path', { d: bez(PW, planH ? TOP + planH / 2 - PG : mid, B.x, B.y + CH / 2), class: 'wk-route dash' }));
      }
      for (const t of g.tasks) {
        const A = pos.get(t.id);
        if (A && !g.edges.some(([from]) => from === t.id)) svg.append(svgEl('path', { d: bez(A.x + CW, A.y + CH / 2, dx, mid), class: 'wk-route dash' }));
      }
    }
    const canvas = h('div.wk-map', { style: `width:${width}px;height:${height}px`, role: 'group', 'aria-label': p ? `The work of ${p.title}` : 'Tasks outside projects' }, svg);
    if (p) {
      plan.forEach((c, i) => canvas.append(planCard(c, p, TOP + i * (PH + PG))));
      canvas.append(deliveryCard(p, { left: dx, top: mid - PH / 2 }));
    }
    const byId = new Map(g.tasks.map((t) => [t.id, t]));
    const cards = cardsOf(g);
    for (const c of laid.cards) {
      const t = byId.get(c.id);
      const at = pos.get(c.id);
      if (t && at) canvas.append(taskCard(t, { left: at.x, top: at.y }, kept !== null && !kept.has(t.id), cards));
    }
    return h('div.wk-scroll', null, canvas);
  }

  /** Tasks in state columns, for a project whose tasks have no links, or while the layout loads. */
  function columnsOf(g: { tasks: GraphTask[]; edges: [string, string][] }, kept: Set<string> | null, notes: string[]): HTMLElement {
    const cards = cardsOf(g);
    return h(
      'div.wk-scroll',
      null,
      notes.map((n) => h('p.wk-note', null, n)),
      h(
        'div.wk-cols',
        null,
        STATE_COLUMNS.map((col) => {
          const items = g.tasks.filter((t) => col.states.includes(stateOf(t)));
          return h(
            'div',
            null,
            h('div.wk-ch', null, h('span', null, col.label), h('span', null, String(items.length))),
            items.length ? items.map((t) => taskCard(t, null, kept !== null && !kept.has(t.id), cards)) : h('div.wk-none', null, 'None.'),
          );
        }),
      ),
    );
  }

  /** While the project plans, its stages are its columns: the plan, with its documents and its critiques. */
  function planning(p: Project): HTMLElement {
    const cards = planCards(p);
    const docsOf = (kinds: string[]): Node[] =>
      p.docs
        .filter((d) => kinds.includes(d.kind))
        .map((d) => h('div.wk-doc', null, h('span.wk-k', null, d.kind), h('b', null, d.title), h('span.wk-note', null, `${d.by} · ${agoLabel(d.at_ms)}`)));
    const crits: Node[] = p.critiques.map((c) =>
      h(
        'div.wk-doc',
        null,
        h('span.wk-k', null, cap(c.verdict)),
        h('b', null, c.by),
        h('span.wk-note', null, `revision ${c.revision} · ${agoLabel(c.at_ms)}`),
        h('span.wk-note', null, c.text),
      ),
    );
    const col = (c: PlanCard, body: Node[], idle: string): HTMLElement =>
      h(
        'section',
        { class: c.state === 'current' || c.state === 'you' ? 'p-current' : c.state === 'done' ? 'p-done' : 'p-later', 'aria-label': c.title },
        h(
          'div.wk-pt',
          null,
          c.title,
          c.state === 'current' ? [' ', badge('Now', 'accent')] : null,
          c.state === 'you' ? [' ', badge('you', 'accent')] : null,
        ),
        h('div.wk-note', null, c.line),
        body.length ? body : h('div.wk-none', null, idle),
      );
    const at = (stage: string): PlanCard => cards.find((c) => c.stage === stage) as PlanCard;
    return h(
      'div',
      null,
      h(
        'div.wk-plan',
        null,
        col(at('brief'), docsOf(['brief']), 'The orchestrator writes the brief.'),
        col(at('research'), docsOf(['research']), 'Researchers attach what they find.'),
        col(at('proposal'), docsOf(['proposal', 'design', 'spec', 'tasks', 'revision']), 'The planner writes the proposal, the design and the tasks list.'),
        col(at('critique'), crits, 'Agents other than the author critique the plan.'),
        col(at('approval'), [], 'The orchestrator approves.'),
      ),
      h(
        'section.wk-note',
        { 'aria-label': 'Tasks', style: 'margin-top:8px' },
        'Tasks: Posted from the plan’s tasks list once it is approved, each after the tasks it names.',
      ),
    );
  }

  // ---- The drawer: the task's lifecycle ----

  /** The line of a task's card, as the drawer says it too: the card's words, with the titles of the tasks it names. */
  function lineOf(g: { tasks: GraphTask[]; edges: [string, string][] }, t: GraphTask, state: WorkState): string {
    const byId = new Map(g.tasks.map((x) => [x.id, x]));
    return cardLine(t, state, { edges: g.edges, stateOf: (id) => byId.get(id)?.state, taskName: (id) => id, readyAt, nameOf, stepOf, now: Date.now() });
  }

  function drawer(g: { tasks: GraphTask[]; edges: [string, string][] }): HTMLElement | null {
    const t = g.tasks.find((x) => x.id === selected);
    if (!t) return null;
    const state = stateOf(t);
    const byId = new Map(g.tasks.map((x) => [x.id, x]));
    const full = tasks.find((x) => x.id === t.id);
    const after = upstreamOf(t.id, g.edges);
    const note = detail && detail.id === t.id ? (detail.note || detail.outcome || '') : '';
    const first = note.split('\n')[0];
    const steps: Node[] = [];
    const step = (mark: string, text: Node | string): void => {
      steps.push(h('li', null, h('span', null, mark), h('span', null, text)));
    };
    if (full) step('·', `Posted ${agoLabel(full.created_at)}`);
    if (after.length) {
      step(
        '↳',
        h(
          'span',
          null,
          'After ',
          after.flatMap((id, i) => {
            const up = byId.get(id);
            const link = h('button.wk-link', { type: 'button', onclick: () => select(id) }, up ? `${id} ${titleOf(up.goal)} (${STATE_LABEL[stateOf(up)]})` : id);
            return [i ? ', ' : '', link];
          }),
        ),
      );
    }
    if (t.agent_cid !== null) {
      step('→', h('span', null, 'Claimed by ', h('a', { href: href('sessions', t.agent_cid) }, nameOf(t.agent_cid))));
      if (state === 'working' || state === 'needs-you') step('●', lineOf(g, t, state));
      if (state === 'needs-you') step('!', h('a', { href: href('approvals') }, 'Decide in Approvals'));
    } else if (state === 'ready') {
      const at = readyAt(t.id);
      step('○', `Waits for a free ${t.role ? roleLabel(t.role) : 'agent'}${at === undefined ? '' : ` · ${shortSpan(Date.now() - at)}`}`);
    }
    if (state === 'review') step('●', lineOf(g, t, state));
    if (state === 'blocked') step('×', `Blocked: ${t.blocked_by.join(', ')} did not finish. The orchestrator relinks it, posts a replacement, or withdraws it.`);
    if (state === 'done') step('✓', `Done${first ? ` · ${first}` : ''}`);
    if (state === 'failed' || state === 'withdrawn') step('×', `${STATE_LABEL[state]}${first ? ` · ${first}` : ''}`);
    const close = h('button.iconbtn', { type: 'button', title: 'Close', 'aria-label': 'Close', onclick: () => select('') }, lu('x', 16));
    return h(
      'aside.wk-drawer',
      { 'aria-label': `Task ${t.id}` },
      h('div.wk-id', null, h('span', null, t.id), close),
      h('div.wk-dt', null, titleOf(t.goal)),
      h('div', null, badge(STATE_LABEL[state], TONE[state], true), ' ', badge(t.role ? roleLabel(t.role) : 'Any role', 'muted')),
      h('ol', null, steps),
      h('a', { href: href('tasks', t.id) }, 'Open the task'),
    );
  }

  // ---- Drawing ----

  /** The map, or the state columns when the tasks have no links; with "Only what needs me" on and nothing waiting on
   *  the owner, a line says so above them. */
  function drawing(g: { tasks: GraphTask[]; edges: [string, string][] }, p: Project | null): Node {
    const states = new Map(g.tasks.map((t) => [t.id, stateOf(t)]));
    const kept = lens ? needsMe(states, g.edges) : null;
    const nothing = lens && kept !== null && kept.size === 0 ? `Nothing ${p ? `in ${p.title}` : 'outside projects'} needs you now.` : '';
    if (!g.edges.length) {
      return columnsOf(g, kept, [nothing, 'These tasks have no after links yet, so they stand in state columns. A plan’s tasks list gives them their order.'].filter(Boolean));
    }
    const laid = layoutOf(g);
    if (!laid) {
      const why = layoutError ? `The layout did not load (${layoutError}), so the tasks stand in state columns.` : 'Loading the layout…';
      return columnsOf(g, kept, [nothing, why].filter(Boolean));
    }
    const map = mapOf(g, p, kept, laid);
    return nothing ? h('div', null, h('p.wk-note', null, nothing), map) : map;
  }

  function body(): Node {
    if (current === LOOSE) {
      const g = shown();
      if (!g.tasks.length) return emptyBox('workflow', 'No tasks outside projects');
      return drawing(g, null);
    }
    const p = projectOf(current);
    if (!p) return emptyBox('x', 'No such project', 'It may have been removed.');
    const g = shown();
    const building = PROJECT_STAGES.indexOf(p.stage) >= PROJECT_STAGES.indexOf('implementation');
    if (!building || !g.tasks.length) return planning(p);
    return drawing(g, p);
  }

  function draw(): void {
    if (!loaded) return;
    if (!projects.length && !tasks.some((t) => !t.project)) {
      replace(
        screen,
        head(false),
        emptyBox('workflow', 'No projects yet', 'The orchestrator opens a project for each goal you give it; its work shows here.', button('Tell the orchestrator', () => go('overview'), { kind: 'primary' })),
      );
      return;
    }
    const g = shown();
    const side = drawer(g);
    replace(
      screen,
      head(true),
      h(
        'div.wk-page',
        null,
        strip(),
        h('div.wk-body', { class: side ? 'with-drawer' : null }, h('div.wk-main', { style: 'min-width:0' }, body()), side),
      ),
    );
  }

  function head(withLens: boolean): HTMLElement {
    return pageHead(
      'Work',
      SUB,
      withLens
        ? checkBox(lens, 'Only what needs me', (on) => {
            lens = on;
            writeLens(on);
            draw();
          })
        : null,
    );
  }

  function select(id: string): void {
    selected = id;
    detail = null;
    draw();
    if (id) {
      // The drawer narrows the map, and a link in it can name a task scrolled out of sight: bring its card back.
      screen.querySelector(`.wk-card[data-id="${CSS.escape(id)}"]`)?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
      void (async () => {
        const t = await attempt(() => api.task(id));
        if (!alive || selected !== id || !t) return;
        detail = t;
        draw();
      })();
    }
  }

  // ---- Data ----

  const reload = async (): Promise<void> => {
    const [ps, ts] = await Promise.all([attempt(() => api.projects()), attempt(() => api.tasks())]);
    if (!alive) return;
    if (ps) projects = ps.slice().sort((a, b) => Number(a.stage === 'delivered') - Number(b.stage === 'delivered') || b.updated_ms - a.updated_ms);
    if (ts) tasks = ts;
    if (!current) {
      current = wanted || projects.find((p) => p.stage !== 'delivered')?.id || projects[0]?.id || (tasks.some((t) => !t.project) ? LOOSE : '');
    }
    if (current && current !== LOOSE && projectOf(current)) {
      const g = await attempt(() => api.projectGraph(current));
      if (!alive) return;
      graph = g ?? null;
    } else graph = null;
    const g = shown();
    if (g.edges.length && !layouter && !layoutError) {
      try {
        layouter = await loadGraph();
      } catch (e) {
        layoutError = (e as Error).message;
      }
      if (!alive) return;
    }
    loaded = true;
    draw();
  };

  const schedule = (): void => {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => void reload(), DEBOUNCE_MS);
  };

  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape' && selected && !document.querySelector('.scrim')) select('');
  };

  // Steps move many times a minute: only the cards of that agent are drawn again, in place. A step into or out of an
  // owner's decision changes a task's state, and with it the lens, the links and the strip, so that one draws it all;
  // so does a step of the task the drawer shows.
  const onStep = (data: unknown): void => {
    const { cid, step } = data as { cid: number; step: AgentStep | null };
    const decided = (stepOf(cid)?.stage === 'approval') !== (step?.stage === 'approval');
    steps.set(cid, step);
    const g = shown();
    const byId = new Map(g.tasks.map((t) => [t.id, t]));
    const open = byId.get(selected);
    if (decided || (open && (open.agent_cid === cid || open.reviewer_cid === cid))) return draw();
    const cards = cardsOf(g);
    screen.querySelectorAll<HTMLElement>(`[data-cid="${cid}"], [data-reviewer="${cid}"]`).forEach((el) => {
      const t = byId.get(el.dataset.id ?? '');
      if (!t) return;
      const pos = el.style.left ? { left: parseFloat(el.style.left), top: parseFloat(el.style.top) } : null;
      el.replaceWith(taskCard(t, pos, el.classList.contains('wk-faded'), cards));
    });
  };

  const offs = [
    on('project', schedule),
    on('task', schedule),
    on('task_link', schedule),
    on('agent_step', onStep),
    agents.watch(() => draw()),
  ];
  // The cards' times ("4 min ago") age while nothing else moves.
  const ticker = window.setInterval(() => {
    if (!document.hidden) draw();
  }, 30_000);
  document.addEventListener('keydown', onKey);
  void reload();

  return () => {
    alive = false;
    window.clearTimeout(timer);
    window.clearInterval(ticker);
    offs.forEach((off) => off());
    document.removeEventListener('keydown', onKey);
  };
}
