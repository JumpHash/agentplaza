// Work: a project as one flow. Its planning (brief, research, proposal, critique, approval), then its tasks in the
// order their `after` links let them run, then delivery. The tasks are laid out by crates/apz-graph (the wasm in
// src/graph), the same layout the desktop draws; each card says who has the task and what they are doing now, or what
// it waits for. "Only what needs me" keeps the decisions and the work they hold up. Cards never move while a project's
// graph is unchanged, and done work dims in place. Tasks outside projects have a tab of their own.

import '../styles/work.css';
import { scope } from '../scope';
import { api, PROJECT_STAGES, type AgentStep, type GraphTask, type Project, type ProjectGraph, type Task } from '../api';
import { loadGraph, type GraphLayout, type Layout } from '../graph/wasm';
import { agentName, roleLabel } from '../roles';
import { href } from '../router';
import { agents, on } from '../store';
import { ago, attempt, badge, empty, type Tone } from '../ui/components';
import { h, icon, replace } from '../ui/dom';
import type { Cleanup } from './index';
import {
  cardLine,
  claimant,
  compose,
  ended,
  FLOW_TASKS,
  flowSegments,
  looseGraph,
  needsMe,
  pathOf,
  planCards,
  readySince,
  shortSpan,
  STATE_COLUMNS,
  STATE_LABEL,
  titleOf,
  upstreamOf,
  waitsForOwner,
  workState,
  type MapGeometry,
  type PlanCard,
  type Seg,
  type WorkState,
} from './work-model';

const DEBOUNCE_MS = 300;
/** The tab of the tasks outside projects. */
const LOOSE = 'outside';
/** The desk sizes: task cards, the Plan column and delivery. A card is tall enough for a two-line
 *  title between its id row, its line and its state. */
const CARD = { card_width: 232, card_height: 124, gap_x: 48, gap_y: 14, pad: 12 };
const PLAN_W = 196;
const PLAN_H = 62;
const PLAN_GAP = 8;
const SIDE_GAP = 56;
const DELIVERY = { width: 196, height: 96 };
const SVG = 'http://www.w3.org/2000/svg';
const LENS_KEY = 'apz.work.lens';

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

const stageLabel = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);

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

/** A flow line: the plan's squares, a square per task, then review and delivery. */
export function flowLine(segs: Seg[], more = 0): HTMLElement {
  return h(
    'span.wk-flow',
    { 'aria-hidden': 'true' },
    segs.map((s) => h('span.wk-sq', { class: `k-${s.kind} s-${s.state}`, title: s.label })),
    more > 0 ? h('span.wk-more', null, `+${more}`) : null,
  );
}

function svgEl<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string>): SVGElementTagNameMap[K] {
  const el = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  return el;
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  // A tab named in the route, else the project the console is scoped to.
  const wanted = params[0] ?? scope.get() ?? '';
  const screen = h('div.work-view', null);
  root.append(screen);

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
    return a ? agentName(a) : `agent ${cid}`;
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

  function strip(): HTMLElement {
    const loose = tasks.filter((t) => !t.project);
    const count = (p: Project | null, own: Task[]): string =>
      waitsForOwner(p, own, stepOf) ? 'waits for you' : `${own.length} ${own.length === 1 ? 'task' : 'tasks'}`;
    const tab = (key: string, title: string, sub: string, segs: Seg[], more: number): HTMLElement =>
      h(
        'a.wk-tab',
        { href: href('work', key), 'aria-current': key === current ? 'page' : null, 'data-key': `tab-${key}` },
        h('span.wk-tab-title', null, title),
        h('span.wk-tab-sub', null, sub),
        flowLine(segs, more),
      );
    return h(
      'nav.wk-strip',
      { 'aria-label': 'Projects' },
      projects.map((p) => {
        const own = tasksOf(p);
        return tab(p.id, p.title, `${stageLabel(p.stage)} · ${count(p, own)}`, flowSegments(p, own, stepOf), Math.max(0, own.length - FLOW_TASKS));
      }),
      loose.length ? tab(LOOSE, 'Outside projects', count(null, loose), flowSegments(null, loose, stepOf), Math.max(0, loose.length - FLOW_TASKS)) : null,
    );
  }

  // ---- Cards ----

  /** What the cards of one drawing share: the links, and the hub's state of each task, built once, not per card. */
  interface Cards {
    edges: [string, string][];
    hubState: Map<string, string>;
  }
  const cardsOf = (g: { tasks: GraphTask[]; edges: [string, string][] }): Cards => ({ edges: g.edges, hubState: new Map(g.tasks.map((t) => [t.id, t.state])) });

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
    const card = h(
      'button.wk-card',
      {
        type: 'button',
        class: [`st-${state}`, ended(state) ? 'ended' : '', faded ? 'faded' : '', t.id === selected ? 'selected' : ''].filter(Boolean).join(' '),
        style: pos ? `left:${pos.left}px;top:${pos.top}px;width:${CARD.card_width}px;height:${CARD.card_height}px` : null,
        'data-key': `task-${t.id}`,
        'data-task': t.id,
        'data-cid': t.agent_cid ?? '',
        'data-reviewer': t.reviewer_cid ?? '',
        'aria-label': `${t.id}: ${titleOf(t.goal)}. ${STATE_LABEL[state]}. ${line}`,
        'aria-pressed': t.id === selected ? 'true' : 'false',
      },
      h('span.wk-card-top', null, h('span.wk-id', null, t.id), h('span.wk-role', null, t.role ? roleLabel(t.role) : 'Any role')),
      h('strong.wk-card-title', null, titleOf(t.goal)),
      h('span.wk-line', null, line),
      h('span.wk-card-foot', null, badge(STATE_LABEL[state], TONE[state], true)),
    );
    card.addEventListener('click', () => select(t.id));
    return card;
  }

  function planCard(c: PlanCard, top: number): HTMLElement {
    return h(
      'a.wk-plan-card',
      {
        href: href('projects', current),
        class: `p-${c.state}`,
        style: `top:${top}px;width:${PLAN_W}px;height:${PLAN_H}px`,
        title: `${c.title}: ${c.line}`,
      },
      h('span.wk-plan-title', null, c.state === 'done' ? h('span.wk-tick', { 'aria-hidden': 'true' }, '✓') : null, c.title),
      h('span.wk-plan-line', null, c.line),
    );
  }

  function deliveryCard(p: Project, box: { x: number; y: number; width: number; height: number }): HTMLElement {
    const delivered = p.stage === 'delivered';
    return h(
      'div.wk-delivery',
      { class: delivered ? 'p-done' : p.stage === 'review' ? 'p-current' : 'p-later', style: `left:${box.x}px;top:${box.y}px;width:${box.width}px;height:${box.height}px` },
      h('span.wk-plan-title', null, delivered ? 'Delivered' : 'Review, then delivered'),
      h('span.wk-plan-line', null, delivered ? 'The lead attached the deliverable' : 'Once every task is accepted, the lead attaches the deliverable'),
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

  function linkedMap(g: { tasks: GraphTask[]; edges: [string, string][] }, p: Project | null, laid: Layout): HTMLElement {
    const plan = p ? planCards(p) : null;
    const planHeight = plan ? plan.length * PLAN_H + (plan.length - 1) * PLAN_GAP : 0;
    const geo: MapGeometry = compose(laid, {
      plan: plan ? { width: PLAN_W, height: planHeight, anchor: planHeight - PLAN_H / 2 } : null,
      delivery: p ? DELIVERY : null,
      gap: SIDE_GAP,
    });
    const states = new Map(g.tasks.map((t) => [t.id, stateOf(t)]));
    const kept = lens ? needsMe(states, g.edges) : null;
    const canvas = h('div.wk-canvas', { style: `width:${geo.width}px;height:${geo.height}px` });
    const svg = svgEl('svg', { class: 'wk-links', width: String(geo.width), height: String(geo.height), 'aria-hidden': 'true' });
    for (const d of geo.posted) svg.append(svgEl('path', { d, class: 'posted' }));
    for (const d of geo.delivered) svg.append(svgEl('path', { d, class: 'posted' }));
    for (const r of laid.routes) {
      const to = states.get(r.to);
      const dim = to !== undefined && ended(to);
      const faded = kept !== null && !(kept.has(r.from) && kept.has(r.to));
      svg.append(svgEl('path', { d: pathOf(r.points, geo.graph.x, geo.graph.y), class: [dim ? 'ended' : '', faded ? 'faded' : ''].filter(Boolean).join(' ') }));
      const last = r.points[r.points.length - 1];
      if (last) {
        const [x, y] = [last[0] + geo.graph.x, last[1] + geo.graph.y];
        svg.append(svgEl('path', { d: `M${x - 6} ${y - 4}L${x} ${y}L${x - 6} ${y + 4}`, class: ['head', dim ? 'ended' : '', faded ? 'faded' : ''].filter(Boolean).join(' ') }));
      }
    }
    canvas.append(svg);
    if (plan && geo.plan) {
      const column = h('div.wk-plan', { style: `left:0;top:${geo.plan.y}px;width:${PLAN_W}px;height:${geo.plan.height}px`, role: 'group', 'aria-label': 'The plan' });
      plan.forEach((c, i) => column.append(planCard(c, i * (PLAN_H + PLAN_GAP))));
      canvas.append(column);
    }
    const byId = new Map(g.tasks.map((t) => [t.id, t]));
    const cards = cardsOf(g);
    for (const c of laid.cards) {
      const t = byId.get(c.id);
      if (t) canvas.append(taskCard(t, { left: geo.graph.x + c.x, top: geo.graph.y + c.y }, kept !== null && !kept.has(t.id), cards));
    }
    if (p && geo.delivery) canvas.append(deliveryCard(p, geo.delivery));
    return h('div.wk-map', { role: 'group', 'aria-label': p ? `The work of ${p.title}` : 'Tasks outside projects' }, canvas);
  }

  /** Tasks without links stand in state columns, with a line saying why. */
  function stateColumns(g: { tasks: GraphTask[]; edges: [string, string][] }, why: string): HTMLElement {
    const states = new Map(g.tasks.map((t) => [t.id, stateOf(t)]));
    const kept = lens ? needsMe(states, g.edges) : null;
    const cards = cardsOf(g);
    return h(
      'div.wk-flat',
      null,
      h('p.wk-why', null, why),
      h(
        'div.wk-cols',
        null,
        STATE_COLUMNS.map((col) => {
          const items = g.tasks.filter((t) => col.states.includes(states.get(t.id) as WorkState));
          return h(
            'section.wk-col',
            null,
            h('header.wk-col-head', null, h('h2', null, col.label), h('span.wk-count', null, String(items.length))),
            items.length
              ? items.map((t) => taskCard(t, null, kept !== null && !kept.has(t.id), cards))
              : h('p.wk-none', null, 'None.'),
          );
        }),
      ),
    );
  }

  /** While the project plans, its stages are the columns: that is where the work is. */
  function planning(p: Project): HTMLElement {
    const at = PROJECT_STAGES.indexOf(p.stage);
    const docsOf = (kinds: string[]) => p.docs.filter((d) => kinds.includes(d.kind));
    const col = (stage: string, cards: HTMLElement[], idle: string): HTMLElement => {
      const i = PROJECT_STAGES.indexOf(stage as Project['stage']);
      return h(
        'section.wk-stage',
        { class: i < at ? 'p-done' : i === at ? 'p-current' : 'p-later' },
        h('header.wk-col-head', null, h('h2', null, stageLabel(stage)), i === at ? badge('Now', 'accent') : null),
        cards.length ? cards : h('p.wk-none', null, idle),
      );
    };
    const doc = (d: Project['docs'][number]): HTMLElement =>
      h('article.wk-doc', null, h('span.wk-doc-kind', null, d.kind), h('strong', null, d.title), h('span.wk-meta', null, `${d.by} · ${ago(d.at_ms)}`));
    const plan = planCards(p);
    const approval = plan[plan.length - 1];
    return h(
      'div.wk-stages',
      null,
      col('brief', docsOf(['brief']).map(doc), 'The orchestrator writes the brief.'),
      col('research', docsOf(['research']).map(doc), 'Researchers attach what they find.'),
      col('proposal', docsOf(['proposal', 'design', 'spec', 'tasks', 'revision']).map(doc), 'The planner writes the proposal, the design and the tasks list.'),
      col(
        'critique',
        p.critiques.map((c) =>
          h(
            'article.wk-doc',
            { class: `v-${c.verdict}` },
            h('span.wk-doc-kind', null, c.verdict === 'approve' ? 'Approves' : c.verdict === 'revise' ? 'Asks for a revision' : 'Comments'),
            h('strong', null, c.by),
            h('span.wk-meta', null, `revision ${c.revision} · ${ago(c.at_ms)}`),
          ),
        ),
        'Agents other than the author critique the plan.',
      ),
      col(
        'approval',
        approval
          ? [h('article.wk-doc', { class: approval.state === 'you' ? 'v-you' : null }, h('strong', null, approval.title), h('span.wk-meta', null, approval.line))]
          : [],
        'The mayor approves.',
      ),
      h(
        'section.wk-stage.p-later',
        null,
        h('header.wk-col-head', null, h('h2', null, 'Tasks')),
        h('p.wk-none', null, 'Posted from the plan’s tasks list once it is approved, each after the tasks it names.'),
      ),
    );
  }

  // ---- The drawer: the task's lifecycle ----

  function drawer(g: { tasks: GraphTask[]; edges: [string, string][] }): HTMLElement | null {
    const t = g.tasks.find((x) => x.id === selected);
    if (!t) return null;
    const state = stateOf(t);
    const byId = new Map(g.tasks.map((x) => [x.id, x]));
    const after = upstreamOf(t.id, g.edges);
    const steps: Node[] = [];
    const step = (mark: string, text: Node | string, tone = ''): void => {
      steps.push(h('li', { class: tone || null }, h('span.wk-mark', { 'aria-hidden': 'true' }, mark), h('span', null, text)));
    };
    step('·', detail && detail.id === t.id ? `Posted ${ago(detail.created_at)}` : 'Posted', 'done');
    if (after.length) {
      step(
        '↳',
        h(
          'span',
          null,
          'After ',
          after.flatMap((id, i) => {
            const up = byId.get(id);
            const s = up ? stateOf(up) : null;
            const link = h('button.wk-link', { type: 'button', onclick: () => select(id) }, up ? `${id} ${titleOf(up.goal)}` : id);
            return [i ? ', ' : '', link, s ? ` (${STATE_LABEL[s].toLowerCase()})` : ''];
          }),
        ),
        after.every((id) => byId.get(id)?.state === 'done') ? 'done' : '',
      );
    }
    if (t.agent_cid !== null) {
      step('→', h('span', null, 'Claimed by ', h('a', { href: href('agents', t.agent_cid) }, nameOf(t.agent_cid))), 'done');
      if (state === 'working' || state === 'needs-you') {
        const titled = (id: string): string => {
          const up = byId.get(id);
          return up ? titleOf(up.goal) : id;
        };
        const now = cardLine(t, state, { edges: g.edges, stateOf: (id) => byId.get(id)?.state, taskName: titled, nameOf, stepOf, now: Date.now() });
        step('●', now, state === 'needs-you' ? 'you' : 'now');
        if (state === 'needs-you') step('!', h('a', { href: href('approvals') }, 'Decide in Approvals'), 'you');
      }
    } else if (state === 'ready') {
      const at = readyAt(t.id);
      step('○', `Waits for a free ${claimant(t.role)}${at === undefined ? '' : ` · ${shortSpan(Date.now() - at)}`}`, 'now');
    }
    if (state === 'review') step('●', t.reviewer_cid !== null ? h('span', null, 'In review by ', h('a', { href: href('agents', t.reviewer_cid) }, nameOf(t.reviewer_cid))) : 'Waits for a reviewer', 'now');
    if (ended(state)) {
      const note = detail && detail.id === t.id ? detail.note || detail.outcome : null;
      step(state === 'done' ? '✓' : '×', `${STATE_LABEL[state]}${note ? ` · ${note.split('\n')[0]}` : ''}`, state === 'done' ? 'done' : 'bad');
    }
    if (state === 'blocked') step('×', `Blocked: ${t.blocked_by.join(', ')} did not finish. The orchestrator relinks it, posts a replacement, or withdraws it.`, 'bad');
    const close = h('button.wk-close', { type: 'button', 'aria-label': 'Close', onclick: () => select('') }, icon('x'));
    return h(
      'aside.wk-drawer',
      { 'aria-label': `Task ${t.id}` },
      h('header.wk-drawer-head', null, h('span.wk-id', null, t.id), close),
      h('h2.wk-drawer-title', null, titleOf(t.goal)),
      h('div.wk-drawer-badges', null, badge(STATE_LABEL[state], TONE[state], true), t.role ? badge(roleLabel(t.role), 'muted') : null),
      h('ol.wk-life', null, steps),
      h('a.wk-open', { href: href('tasks', t.id) }, 'Open the task', icon('chevronRight')),
    );
  }

  // ---- Drawing ----

  function body(): Node {
    if (current === LOOSE) {
      const g = shown();
      if (!g.tasks.length) return empty('tasks', 'No tasks outside projects');
      return mapOrColumns(g, null);
    }
    const p = projectOf(current);
    if (!p) return empty('projects', 'No such project', 'It may have been removed.');
    const g = shown();
    const building = PROJECT_STAGES.indexOf(p.stage) >= PROJECT_STAGES.indexOf('implementation');
    if (!building || !g.tasks.length) return planning(p);
    return mapOrColumns(g, p);
  }

  /** The map or the columns; with the lens on and nothing waiting on the owner, a line says so above them. */
  function mapOrColumns(g: { tasks: GraphTask[]; edges: [string, string][] }, p: Project | null): Node {
    const shown = mapOrColumnsOf(g, p);
    if (!lens || needsMe(new Map(g.tasks.map((t) => [t.id, stateOf(t)])), g.edges).size) return shown;
    return h('div.wk-lensed', null, h('p.wk-why', null, `Nothing ${p ? `in ${p.title}` : 'outside projects'} needs you now.`), shown);
  }

  function mapOrColumnsOf(g: { tasks: GraphTask[]; edges: [string, string][] }, p: Project | null): Node {
    if (!g.edges.length) return stateColumns(g, 'These tasks have no after links yet, so they stand in state columns. A plan’s tasks list gives them their order.');
    const laid = layoutOf(g);
    if (!laid) {
      const why = layoutError ? `The layout did not load (${layoutError}), so the tasks stand in state columns.` : 'Loading the layout…';
      return stateColumns(g, why);
    }
    return linkedMap(g, p, laid);
  }

  function draw(): void {
    if (!loaded) return;
    if (!projects.length && !tasks.some((t) => !t.project)) {
      replace(
        screen,
        head(),
        empty('projects', 'No projects yet', 'The orchestrator opens a project for each goal you give it; its work shows here.', h('a.btn', { href: href('overview') }, 'Tell the orchestrator')),
      );
      return;
    }
    const g = shown();
    const side = drawer(g);
    replace(screen, head(), strip(), h('div.wk-body', { class: side ? 'with-drawer' : null }, body(), side));
  }

  function head(): HTMLElement {
    const toggle = h('input', { type: 'checkbox', checked: lens || null }) as HTMLInputElement;
    toggle.addEventListener('change', () => {
      lens = toggle.checked;
      writeLens(lens);
      draw();
    });
    return h(
      'div.view-head',
      null,
      h('div', null, h('h1', null, 'Work'), h('p.wk-intro', null, 'A project as one flow: its plan, its tasks in the order they can run, and delivery.')),
      h('label.wk-lens', null, toggle, 'Only what needs me'),
    );
  }

  function select(id: string): void {
    selected = id;
    detail = null;
    draw();
    if (id) {
      // The drawer narrows the map, and a link in it can name a task scrolled out of sight: bring its card back.
      screen.querySelector(`.wk-card[data-task="${CSS.escape(id)}"]`)?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
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
    if (e.key === 'Escape' && selected && !document.querySelector('.modal-backdrop')) select('');
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
      const t = byId.get(el.dataset.task ?? '');
      if (!t) return;
      const pos = el.style.left ? { left: parseFloat(el.style.left), top: parseFloat(el.style.top) } : null;
      el.replaceWith(taskCard(t, pos, el.classList.contains('faded'), cards));
    });
  };

  const offs = [
    on('project', schedule),
    on('task', schedule),
    on('task_link', schedule),
    on('agent_step', onStep),
    agents.watch(() => draw()),
  ];
  // The steps' times ("· 2m") age while nothing else moves.
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
    screen.remove();
  };
}
