// The review of one task's change, for the owner: a radial graph (the task in the middle, the four readings as a ring,
// the change's intents around it, an intent opened into its definitions, a definition into what it touches), with the
// words in a small side panel. Below it, the review's steps in order with a pulse on the running one. Expert: the step
// table, the score board, the findings and the gate's reasons. Polls the hub (the store does not carry review frames).

import '../styles/review.css';
import { review, type RecipeDetail, type ReviewNode, type ReviewRun, type ReviewRunDetail, type ReviewScore, type ReviewTree } from '../api-review';
import { agentName, roleLabel } from '../roles';
import { agents } from '../store';
import { badge, dot, empty, table, tabs, type Column, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import {
  allFindings,
  arcPath,
  arcSpan,
  axisName,
  basisLabel,
  cellOf,
  indexTree,
  intentAngle,
  intentRadius,
  intentTone,
  intentViews,
  kindLabel,
  levelWord,
  lowestFor,
  nameOf,
  nodeTone,
  ownerTone,
  pathOf,
  polar,
  READING_AXES,
  readingTone,
  RADIUS,
  rowX,
  runStateLabel,
  runTone,
  scoreRows,
  scoresOf,
  spread,
  touchesOf,
  wordedText,
  type IntentView,
  type NodeTone,
  type ReadingTone,
  type ScoreRow,
  type TreeIndex,
} from './review-model';

const POLL_MS = 4_000;
const VIEW = 640;
const C = VIEW / 2;
const SVG_NS = 'http://www.w3.org/2000/svg';
const MOTION = { intent: 620, def: 560, touch: 480, out: 380 } as const;

// ---- Per-task memory: the selection survives a redraw of the page that holds the panel ----------------------------

type Sel =
  | { kind: 'none' }
  | { kind: 'reading'; name: string }
  | { kind: 'intent'; key: string }
  | { kind: 'def'; id: string }
  | { kind: 'touch'; from: string; index: number }
  | { kind: 'node'; id: string };

interface Memory { run: string | null; expert: boolean; sel: Sel; open: string | null; lens: string | null }
const NONE: Sel = { kind: 'none' };
const memories = new Map<string, Memory>();

const memoryOf = (taskId: string): Memory => {
  let m = memories.get(taskId);
  if (!m) memories.set(taskId, (m = { run: null, expert: false, sel: NONE, open: null, lens: null }));
  return m;
};

/** The last data the panel drew for a task, so a new panel on the same task shows it at once. */
interface Loaded { list: ReviewRun[]; detail: ReviewRunDetail; tree: ReviewTree; index: TreeIndex; views: IntentView[]; recipe: RecipeDetail | null; stamp: number }
const loadedOf = new Map<string, Loaded>();

// ---- SVG helpers --------------------------------------------------------------------------------------------------

function svgEl<T extends SVGElement = SVGElement>(tag: string, attrs: Record<string, string | number> = {}, cls?: string): T {
  const el = document.createElementNS(SVG_NS, tag) as T;
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, String(v));
  if (cls) el.setAttribute('class', cls);
  return el;
}

function titled(el: SVGElement, text: string): void {
  const t = svgEl('title');
  t.textContent = text;
  el.append(t);
}

/** Makes a graphic element a button: a click or Enter/Space runs `fn`, and a click does not reach the background. */
function activable(g: SVGElement, fn: () => void): void {
  g.setAttribute('tabindex', '0');
  g.setAttribute('role', 'button');
  g.addEventListener('click', (e) => {
    e.stopPropagation();
    fn();
  });
  g.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      fn();
    }
  });
}

const prefersReduced = (): boolean => typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;

interface SpecNode { key: string; x: number; y: number; cls: string; dur: number; delay: number; build: (g: SVGGElement) => void }
interface SpecEdge { key: string; a: string; b: string; cls: string }
interface Placed { g: SVGGElement; fx: number; fy: number; tx: number; ty: number; x: number; y: number; t0: number; dur: number; dying: boolean }

/**
 * The moving part of the radial graph. Each node eases from the centre to its place (and back, when it goes); lines
 * follow their ends. Nodes are kept by key, so a redraw moves them rather than rebuilding them.
 */
class Scene {
  private readonly links: SVGGElement;
  private readonly nodes: SVGGElement;
  private readonly placed = new Map<string, Placed>();
  private readonly lines = new Map<string, { el: SVGLineElement; a: string; b: string }>();
  private frame = 0;
  private gone = false;

  constructor(svg: SVGSVGElement, private readonly cx: number, private readonly cy: number) {
    this.links = svgEl<SVGGElement>('g', {}, 'rv-links');
    this.nodes = svgEl<SVGGElement>('g', {}, 'rv-nodes');
    svg.append(this.links, this.nodes);
  }

  sync(nodes: SpecNode[], edges: SpecEdge[]): void {
    const now = performance.now();
    const quick = prefersReduced();
    const live = new Set<string>();
    for (const spec of nodes) {
      live.add(spec.key);
      const delay = quick ? 0 : spec.delay;
      const dur = quick ? 0 : spec.dur;
      let p = this.placed.get(spec.key);
      if (!p) {
        const g = svgEl<SVGGElement>('g', {}, 'rv-node');
        spec.build(g);
        this.nodes.append(g);
        p = { g, fx: this.cx, fy: this.cy, tx: spec.x, ty: spec.y, x: this.cx, y: this.cy, t0: now + delay, dur, dying: false };
        this.placed.set(spec.key, p);
      } else if (p.dying || p.tx !== spec.x || p.ty !== spec.y) {
        p.fx = p.x;
        p.fy = p.y;
        p.tx = spec.x;
        p.ty = spec.y;
        p.t0 = now + delay;
        p.dur = dur;
        p.dying = false;
      }
      p.g.setAttribute('class', `rv-node ${spec.cls}`);
    }
    for (const [key, p] of this.placed) {
      if (live.has(key) || p.dying) continue;
      p.dying = true;
      p.fx = p.x;
      p.fy = p.y;
      p.tx = this.cx;
      p.ty = this.cy;
      p.t0 = now;
      p.dur = quick ? 0 : MOTION.out;
    }
    const wanted = new Set<string>();
    for (const e of edges) {
      wanted.add(e.key);
      let l = this.lines.get(e.key);
      if (!l) {
        const el = svgEl<SVGLineElement>('line', {}, 'rv-link');
        this.links.append(el);
        l = { el, a: e.a, b: e.b };
        this.lines.set(e.key, l);
      }
      l.a = e.a;
      l.b = e.b;
      l.el.setAttribute('class', `rv-link ${e.cls}`);
    }
    for (const [key, l] of this.lines) {
      if (wanted.has(key)) continue;
      l.el.remove();
      this.lines.delete(key);
    }
    this.step(now);
    this.kick();
  }

  destroy(): void {
    this.gone = true;
    cancelAnimationFrame(this.frame);
  }

  private kick(): void {
    if (!this.frame && !this.gone) this.frame = requestAnimationFrame((t) => this.tick(t));
  }

  private tick(now: number): void {
    this.frame = 0;
    if (this.gone) return;
    if (this.step(now)) this.kick();
  }

  /** Places every node for `now` and redraws the lines; true while something still moves. */
  private step(now: number): boolean {
    let moving = false;
    for (const [key, p] of this.placed) {
      const t = p.dur <= 0 ? 1 : Math.min(1, Math.max(0, (now - p.t0) / p.dur));
      const k = 1 - Math.pow(1 - t, 3);
      p.x = p.fx + (p.tx - p.fx) * k;
      p.y = p.fy + (p.ty - p.fy) * k;
      p.g.setAttribute('transform', `translate(${p.x.toFixed(1)} ${p.y.toFixed(1)})`);
      p.g.style.opacity = p.dying ? String(1 - t) : now < p.t0 ? '0' : '1';
      if (p.dying && t >= 1) {
        p.g.remove();
        this.placed.delete(key);
        continue;
      }
      if (t < 1) moving = true;
    }
    for (const l of this.lines.values()) {
      const [x1, y1] = this.at(l.a);
      const [x2, y2] = this.at(l.b);
      l.el.setAttribute('x1', x1.toFixed(1));
      l.el.setAttribute('y1', y1.toFixed(1));
      l.el.setAttribute('x2', x2.toFixed(1));
      l.el.setAttribute('y2', y2.toFixed(1));
    }
    return moving;
  }

  private at(key: string): [number, number] {
    if (key === 'c') return [this.cx, this.cy];
    const p = this.placed.get(key);
    return p ? [p.x, p.y] : [this.cx, this.cy];
  }
}

// ---- Words for the side panel ---------------------------------------------------------------------------------------

const toneOfReading = (t: ReadingTone): Tone => (t === 'better' ? 'ok' : t === 'worse' ? 'danger' : 'muted');
const stepLabel = (state: string): string => runStateLabel(state);
const scoreText = (s: ReviewScore): string => `${axisName(s.axis)}: ${s.level} · ${levelWord(s.level)} · ${basisLabel(s.basis)}`;

/** The agent's name for an assignee id, or `agent N` while the roster does not have it. */
function assigneeName(cid: number | null): string {
  if (cid === null) return '—';
  const a = agents.get().find((x) => x.cid === cid);
  return a ? agentName(a) : `agent ${cid}`;
}

/** A definition's touches and worded touches as one list, in the order the side panel names them. */
interface TouchItem { text: string; to: string; changed: boolean; worded: boolean }
function touchItemsOf(index: TreeIndex, id: string): TouchItem[] {
  const { touches, worded } = touchesOf(index, id);
  return [
    ...touches.map((t) => ({ text: `${t.how} ${nameOf(t.to)}`, to: t.to, changed: t.changed, worded: false })),
    ...worded.map((w) => ({ text: wordedText(w), to: '', changed: false, worded: true })),
  ];
}

/** The steps in the order the flow draws them: the tree, the steps, the gate, then the owner's ruling. */
interface FlowStep { id: string; tone: NodeTone; kind: 'tree' | 'step' | 'gate' | 'owner'; state: string }
function flowStepsOf(run: ReviewRunDetail): FlowStep[] {
  const decision = run.outcome?.decision ?? null;
  const steps: FlowStep[] = [];
  for (const n of run.nodes) {
    if (n.def.kind === 'gate') continue;
    steps.push({ id: n.def.id, tone: nodeTone(n.state), kind: n.def.tool === 'tree' ? 'tree' : 'step', state: n.state });
  }
  for (const n of run.nodes) {
    if (n.def.kind !== 'gate') continue;
    steps.push({ id: n.def.id, tone: decision ? ownerTone(decision) : nodeTone(n.state), kind: 'gate', state: n.state });
  }
  const ruled = run.outcome?.by === 'owner';
  steps.push({ id: 'owner', tone: decision === 'owner' ? 'owner' : ruled ? ownerTone(decision) : 'wait', kind: 'owner', state: decision ?? 'waiting' });
  return steps;
}

/** The flow strip: dots in a row, lines between, a pulse moving to the running one. */
function buildFlow(steps: FlowStep[], pick: (id: string) => void): { el: SVGSVGElement; dots: Map<string, SVGGElement> } {
  const W = VIEW;
  const Y = 40;
  const svg = svgEl<SVGSVGElement>('svg', { viewBox: `0 0 ${W} 80`, role: 'group', 'aria-label': 'The review steps, in order' }, 'rv-flow-svg');
  const xs = steps.map((_, i) => rowX(i, steps.length, 40, W - 40));
  const x = (i: number): number => xs[i] ?? 0;
  for (let i = 1; i < steps.length; i++) svg.append(svgEl('line', { x1: x(i - 1), y1: Y, x2: x(i), y2: Y }, 'rv-flow-link'));
  const running = steps.findIndex((s) => s.tone === 'run');
  if (running > 0) {
    const pulse = svgEl('circle', { r: 3.5 }, 'rv-pulse');
    const motion = svgEl('animateMotion', { dur: '1.6s', repeatCount: 'indefinite', path: `M ${x(running - 1)} ${Y} L ${x(running)} ${Y}` });
    pulse.append(motion);
    svg.append(pulse);
  }
  const dots = new Map<string, SVGGElement>();
  steps.forEach((s, i) => {
    const big = s.kind === 'gate' || s.kind === 'owner';
    const g = svgEl<SVGGElement>('g', { transform: `translate(${x(i)} ${Y})` }, `rv-flow-node st-${s.tone}${s.tone === 'run' || s.tone === 'owner' ? ' pulse' : ''}`);
    g.dataset.base = g.getAttribute('class') ?? '';
    g.append(svgEl('circle', { r: 16 }, 'rv-hit'), svgEl('circle', { r: big ? 10 : 7 }, 'rv-dot'));
    titled(g, `${s.id}: ${stepLabel(s.state)}`);
    activable(g, () => pick(s.id));
    dots.set(s.id, g);
    svg.append(g);
  });
  return { el: svg, dots };
}

function allScoresOf(index: TreeIndex): ReviewScore[] {
  const out: ReviewScore[] = [];
  for (const f of index.tree.files) {
    out.push(...f.scores);
    for (const s of f.symbols) out.push(...s.scores);
  }
  return out;
}

function sectionOf(title: string, ...children: (Node | string | null | false)[]): HTMLElement {
  return h('section.rv-block', null, h('h3', null, title), ...children);
}

function listOf(items: (Node | string)[], empty_ = 'None.'): HTMLElement {
  return items.length ? h('ul.rv-list', null, items.map((i) => h('li', null, i))) : h('p.rv-faint', null, empty_);
}

function findingNode(f: { text: string; severity: string; round: number; symbol: string }): HTMLElement {
  return h(
    'div.rv-finding',
    null,
    h('span.rv-round', null, `Round ${f.round}`),
    h('span.rv-sev', { class: f.severity === 'major' || f.severity === 'blocker' ? 'major' : '' }, f.severity),
    h('span', null, f.text),
    h('span.rv-faint', null, nameOf(f.symbol)),
  );
}

const cellNode = (scores: readonly ReviewScore[], axis: string): HTMLElement => {
  const s = cellOf(scores, axis);
  if (!s) return h('span.rv-faint', null, '·');
  return h(
    'span.rv-cell',
    { class: `tone-${readingTone(s.level)}`, title: `${axisName(axis)}: ${levelWord(s.level)}, ${basisLabel(s.basis)} (confidence ${Math.round(s.confidence * 100)}%). ${s.evidence}` },
    h('span.rv-measure', { class: s.basis === 'measured' ? 'measured' : 'estimated' }),
    String(s.level),
  );
};

// ---- The panel ------------------------------------------------------------------------------------------------------

/**
 * The review of a task's change: its newest run, or the run `runId` names (an approval card's). Keeps itself fresh
 * every few seconds, and stops once it has left the page.
 */
export function reviewPanel(taskId: string, runId?: string): HTMLElement {
  const mem = memoryOf(taskId);
  if (runId && mem.run !== runId) {
    mem.run = runId;
    mem.open = null;
    mem.sel = NONE;
    mem.lens = null;
  }
  const root = h('section.rv', { 'data-task': taskId });
  const headEl = h('div.rv-head');
  const bodyEl = h('div.rv-body');
  const runsEl = h('div.rv-runs');
  root.append(headEl, bodyEl, runsEl);

  const cached = loadedOf.get(taskId) ?? null;
  let alive = true;
  let busy = false;
  let detached = 0;
  let loaded = cached !== null;
  let failed = false;
  let list: ReviewRun[] = cached?.list ?? [];
  let detail: ReviewRunDetail | null = cached?.detail ?? null;
  let tree: ReviewTree | null = cached?.tree ?? null;
  let index: TreeIndex | null = cached?.index ?? null;
  let views: IntentView[] = cached?.views ?? [];
  let recipe: RecipeDetail | null = cached?.recipe ?? null;
  let stamp = cached?.stamp ?? -1;
  let recipeFor = cached?.detail.recipe ?? '';
  let lastKey = '';

  // The drawn graph of the plain view: rebuilt for a new run, moved for a new selection.
  let scene: Scene | null = null;
  let sceneKey = '';
  let sideEl: HTMLElement = h('aside.rv-side');
  let arcs = new Map<string, SVGGElement>();
  let dots = new Map<string, SVGGElement>();
  let flowSteps: FlowStep[] = [];

  const act = (change: () => void): void => {
    change();
    applySelection();
  };

  const clearAll = (): void => {
    mem.open = null;
    mem.lens = null;
    mem.sel = NONE;
  };

  const toggleIntent = (key: string): void => {
    if (mem.open === key) clearAll();
    else {
      mem.open = key;
      mem.sel = { kind: 'intent', key };
    }
  };

  const toggleDef = (id: string): void => {
    if (mem.sel.kind === 'def' && mem.sel.id === id) mem.sel = mem.open ? { kind: 'intent', key: mem.open } : NONE;
    else mem.sel = { kind: 'def', id };
  };

  const toggleLens = (name: string): void => {
    if (mem.lens === name) {
      mem.lens = null;
      mem.sel = NONE;
    } else {
      mem.lens = name;
      mem.sel = { kind: 'reading', name };
    }
  };

  /** Draws the radial graph's nodes and lines for the selection, and the words beside it. */
  const applySelection = (): void => {
    if (!scene || !index || !detail || !tree) return;
    const idx = index;
    const lensAxes = mem.lens ? READING_AXES[mem.lens] ?? [] : null;
    for (const [name, g] of arcs) {
      const level = tree.readings.find((r) => r.name === name)?.level ?? null;
      g.setAttribute('class', `rv-arc tone-${readingTone(level)}${mem.lens === name ? ' on' : ''}`);
    }
    const nodes: SpecNode[] = [];
    const edges: SpecEdge[] = [];
    const angleOf = new Map<string, number>();
    const defAngle = new Map<string, number>();

    views.forEach((v, i) => {
      const angle = intentAngle(i, views.length);
      angleOf.set(v.key, angle);
      const [x, y] = polar(C, C, RADIUS.intent, angle);
      const tone = intentTone(lowestFor(idx, v.items, lensAxes));
      const selected = mem.sel.kind === 'intent' && mem.sel.key === v.key;
      const r = intentRadius(v.items.length);
      const label = `${v.title}: ${v.items.length} ${v.items.length === 1 ? 'definition' : 'definitions'}`;
      nodes.push({
        key: `i:${v.key}`,
        x,
        y,
        dur: MOTION.intent,
        delay: i * 45,
        cls: `k-intent tone-${tone}${v.orphan ? ' orphan' : ''}${mem.open === v.key ? ' open' : ''}${selected ? ' sel' : ''}`,
        build: (g) => {
          g.append(svgEl('circle', { r: 26 }, 'rv-hit'), svgEl('circle', { r }, 'rv-dot'));
          titled(g, label);
          activable(g, () => act(() => toggleIntent(v.key)));
        },
      });
      edges.push({ key: `s:${v.key}`, a: 'c', b: `i:${v.key}`, cls: 'spoke' });
    });

    const open = views.find((v) => v.key === mem.open) ?? null;
    if (open) {
      const base = angleOf.get(open.key) ?? 0;
      const n = open.items.length;
      const angles = spread(base, n, Math.min(120, Math.max(0, (n - 1) * 18)));
      open.items.forEach((id, j) => {
        const angle = angles[j] ?? base;
        defAngle.set(id, angle);
        const [x, y] = polar(C, C, RADIUS.def, angle);
        const tone = intentTone(lowestFor(idx, [id], lensAxes));
        const selected = (mem.sel.kind === 'def' && mem.sel.id === id) || (mem.sel.kind === 'touch' && mem.sel.from === id);
        nodes.push({
          key: `d:${id}`,
          x,
          y,
          dur: MOTION.def,
          delay: 120 + j * 40,
          cls: `k-def tone-${tone}${selected ? ' sel' : ''}`,
          build: (g) => {
            g.append(svgEl('circle', { r: 14 }, 'rv-hit'), svgEl('circle', { r: 6.5 }, 'rv-dot'));
            titled(g, nameOf(id));
            activable(g, () => act(() => toggleDef(id)));
          },
        });
        edges.push({ key: `e:${open.key}:${id}`, a: `i:${open.key}`, b: `d:${id}`, cls: 'def-link' });
      });
    }

    if ((mem.sel.kind === 'def' || mem.sel.kind === 'touch') && defAngle.has(mem.sel.kind === 'def' ? mem.sel.id : mem.sel.from)) {
      const from = mem.sel.kind === 'def' ? mem.sel.id : mem.sel.from;
      const items = touchItemsOf(idx, from);
      const base = defAngle.get(from) ?? 0;
      const angles = spread(base, items.length, Math.min(90, Math.max(0, (items.length - 1) * 12)));
      items.forEach((t, j) => {
        const angle = angles[j] ?? base;
        const [x, y] = polar(C, C, RADIUS.touch, angle);
        const selected = mem.sel.kind === 'touch' && mem.sel.from === from && mem.sel.index === j;
        nodes.push({
          key: `t:${from}:${j}`,
          x,
          y,
          dur: MOTION.touch,
          delay: 60 + j * 30,
          cls: `k-touch${t.worded ? ' worded' : ''}${t.changed ? ' changed' : ''}${selected ? ' sel' : ''}`,
          build: (g) => {
            g.append(svgEl('circle', { r: 9 }, 'rv-hit'), svgEl('circle', { r: 5 }, 'rv-dot'));
            titled(g, t.text);
            activable(g, () =>
              act(() => {
                mem.sel = mem.sel.kind === 'touch' && mem.sel.from === from && mem.sel.index === j ? { kind: 'def', id: from } : { kind: 'touch', from, index: j };
              }),
            );
          },
        });
        edges.push({ key: `o:${from}:${j}`, a: `d:${from}`, b: `t:${from}:${j}`, cls: t.worded ? 'touch-link worded' : 'touch-link' });
      });
    }

    const verdictTone = runTone(detail.state);
    nodes.push({
      key: 'c',
      x: C,
      y: C,
      dur: 0,
      delay: 0,
      cls: `k-task tone-${verdictTone}`,
      build: (g) => {
        g.append(svgEl('circle', { r: RADIUS.task + 6 }, 'rv-hit'), svgEl('circle', { r: RADIUS.task }, 'rv-dot'));
        const label = svgEl('text', { 'text-anchor': 'middle', dy: '0.35em' }, 'rv-id');
        label.textContent = taskId;
        g.append(label);
        titled(g, `${taskId}: ${runStateLabel(detail?.state ?? '')}`);
        activable(g, () => act(clearAll));
      },
    });

    scene.sync(nodes, edges);
    for (const step of flowSteps) {
      const g = dots.get(step.id);
      if (!g) continue;
      const selected = mem.sel.kind === 'node' && mem.sel.id === step.id;
      g.setAttribute('class', `${g.dataset.base ?? ''}${selected ? ' sel' : ''}`);
    }
    replace(sideEl, ...sideContent(idx));
  };

  /** The words for the side panel: what is selected, its scores, its findings. */
  const sideContent = (idx: TreeIndex): (Node | null)[] => {
    if (!detail || !tree) return [];
    const sel = mem.sel;
    if (sel.kind === 'reading') {
      const axes = READING_AXES[sel.name] ?? [];
      const level = tree.readings.find((r) => r.name === sel.name)?.level ?? null;
      const scores = allScoresOf(idx).filter((s) => axes.includes(s.axis)).sort((a, b) => a.level - b.level).slice(0, 10);
      return [
        h('h3', null, sel.name),
        h('p', null, levelWord(level)),
        h('p.rv-faint', null, mem.lens === sel.name ? 'The intents are coloured by this. Tap the segment again to clear it.' : 'Tap the segment to colour the intents by it.'),
        listOf(scores.map((s) => h('span', null, `${nameOf(s.symbol)} · ${scoreText(s)}`)), 'Nothing measured on it yet.'),
      ];
    }
    if (sel.kind === 'intent') {
      const v = views.find((x) => x.key === sel.key);
      if (!v) return defaultContent(idx);
      const findings = v.items.flatMap((id) => idx.symbols.get(id)?.findings ?? []);
      return [
        h('h3', null, v.title),
        h('p.rv-faint', null, [kindLabel(v.kind), `${v.items.length} ${v.items.length === 1 ? 'definition' : 'definitions'}`].filter(Boolean).join(' · ')),
        v.summary ? h('p', null, v.summary) : null,
        listOf(
          v.items.map((id) => h('button.rv-link', { type: 'button', onclick: () => act(() => toggleDef(id)) }, nameOf(id))),
          'Nothing in it.',
        ),
        findings.length ? sectionOf('Findings', ...findings.map((f) => findingNode(f))) : null,
      ];
    }
    if (sel.kind === 'def' || sel.kind === 'touch') {
      const id = sel.kind === 'def' ? sel.id : sel.from;
      const sym = idx.symbols.get(id);
      const scores = scoresOf(idx, id);
      const items = touchItemsOf(idx, id);
      const back = sel.kind === 'touch' ? h('button.rv-link', { type: 'button', onclick: () => act(() => (mem.sel = { kind: 'def', id })) }, `Back to ${nameOf(id)}`) : null;
      const current = sel.kind === 'touch' ? items[sel.index] : null;
      if (sel.kind === 'touch' && current) {
        return [h('h3', null, nameOf(id)), h('p', null, current.text), current.changed ? h('p.rv-faint', null, 'Changed in this change.') : null, back];
      }
      return [
        h('h3', null, nameOf(id)),
        h('p.rv-faint', null, [pathOf(id), sym ? kindLabel(sym.kind.toLowerCase()) : null].filter(Boolean).join(' · ')),
        sectionOf('Scores', listOf(scores.map((s) => h('span', { title: s.evidence }, scoreText(s))), 'Not scored yet.')),
        sym?.findings.length ? sectionOf('Findings', ...sym.findings.map((f) => findingNode(f))) : null,
        sectionOf(
          'Touches',
          listOf(
            items.map((t, j) => h('button.rv-link', { type: 'button', onclick: () => act(() => (mem.sel = { kind: 'touch', from: id, index: j })), class: t.changed ? 'changed' : null }, t.text)),
            'Touches nothing else.',
          ),
        ),
      ];
    }
    if (sel.kind === 'node') return nodeContent(sel.id);
    return defaultContent(idx);
  };

  const defaultContent = (idx: TreeIndex): (Node | null)[] => {
    if (!detail || !tree) return [];
    const reasons = detail.outcome?.reasons ?? [];
    return [
      h('h3', null, 'The change'),
      h('p.rv-faint', null, `${idx.tree.files.length} ${idx.tree.files.length === 1 ? 'file' : 'files'} · tap a dot to open it`),
      h(
        'ul.rv-readings',
        null,
        tree.readings.map((r) =>
          h('li', null, dot(toneOfReading(readingTone(r.level))), h('span', null, r.name), h('span.rv-faint', null, levelWord(r.level))),
        ),
      ),
      reasons.length && detail.outcome?.decision !== 'pass' ? sectionOf('Why', listOf(reasons)) : null,
    ];
  };

  /** The words for one step of the flow, or for the owner's ruling. */
  const nodeContent = (id: string): (Node | null)[] => {
    if (!detail) return [];
    if (id === 'owner') {
      const outcome = detail.outcome;
      return [
        h('h3', null, 'Your call'),
        h('p', null, outcome ? (outcome.decision === 'owner' ? 'The gate sent it to you.' : `Ruled ${runStateLabel(outcome.decision).toLowerCase()}.`) : 'Waiting for the gate.'),
        outcome?.reasons.length ? sectionOf('Why', listOf(outcome.reasons)) : null,
      ];
    }
    const n = detail.nodes.find((x) => x.def.id === id);
    if (!n) return [];
    return [
      h('h3', null, id),
      h('p.rv-faint', null, [n.def.role ? roleLabel(n.def.role) : null, assigneeName(n.assignee)].filter(Boolean).join(' · ')),
      h('p', null, stepLabel(n.state)),
      n.notes ? h('p', null, n.notes) : null,
      n.scores.length ? sectionOf('Scores', listOf(n.scores.map((s) => h('span', { title: s.evidence }, scoreText(s))))) : null,
      n.findings.length ? sectionOf('Findings', ...n.findings.map((f) => findingNode(f))) : null,
    ];
  };

  const pick = (id: string): void => act(() => (mem.sel.kind === 'node' && mem.sel.id === id ? (mem.sel = NONE) : (mem.sel = { kind: 'node', id })));

  /** The plain view: the radial graph with its side panel, and the flow strip under it. */
  const buildPlain = (run: ReviewRunDetail, treeData: ReviewTree): void => {
    scene?.destroy();
    arcs = new Map();
    const svg = svgEl<SVGSVGElement>('svg', { viewBox: `0 0 ${VIEW} ${VIEW}`, role: 'group', 'aria-label': 'The review graph' }, 'rv-svg');
    const ring = svgEl<SVGGElement>('g', {}, 'rv-ring');
    treeData.readings.forEach((r, i) => {
      const { from, to } = arcSpan(i, treeData.readings.length);
      const d = arcPath(C, C, RADIUS.ring, from, to);
      const g = svgEl<SVGGElement>('g', {}, 'rv-arc');
      g.append(svgEl('path', { d }, 'rv-arc-hit'), svgEl('path', { d }, 'rv-arc-seg'));
      titled(g, `${r.name}: ${r.level === null ? 'not measured' : levelWord(r.level)}`);
      activable(g, () => act(() => toggleLens(r.name)));
      arcs.set(r.name, g);
      ring.append(g);
    });
    svg.append(ring);
    svg.addEventListener('click', () => act(clearAll));
    scene = new Scene(svg, C, C);
    sideEl = h('aside.rv-side');

    flowSteps = flowStepsOf(run);
    const flow = buildFlow(flowSteps, pick);
    dots = flow.dots;
    replace(
      bodyEl,
      h('div.rv-plain', null, h('div.rv-stage', null, h('div.rv-canvas', null, svg), sideEl), h('div.rv-flow', null, h('span.rv-faint', null, 'Steps, in order'), flow.el)),
    );
  };

  /** The expert view: the steps, the score board, the findings, the gate's reasons. */
  const expertView = (run: ReviewRunDetail, idx: TreeIndex): Node => {
    const nodeCols: Column<ReviewNode>[] = [
      { label: 'Step', cell: (n) => n.def.id },
      { label: 'Kind', cell: (n) => n.def.kind },
      { label: 'Reviewer', cell: (n) => (n.def.role ? roleLabel(n.def.role) : '—') },
      { label: 'Assignee', cell: (n) => assigneeName(n.assignee) },
      { label: 'State', cell: (n) => stepLabel(n.state) },
      { label: 'Notes', cell: (n) => n.notes || '' },
    ];
    const rows: ScoreRow[] = scoreRows(idx);
    const axes = recipe && recipe.recipe.axes.length ? recipe.recipe.axes : [...new Set(rows.flatMap((r) => r.scores.map((s) => s.axis)))];
    const boardCols: Column<ScoreRow>[] = [
      { label: 'Definition', cell: (r) => h('span.rv-name', { title: r.id }, r.name) },
      ...axes.map((a) => ({ label: axisName(a), num: true, cell: (r: ScoreRow) => cellNode(r.scores, a) })),
    ];
    const findings = allFindings(idx).sort((a, b) => b.round - a.round);
    const gate = run.nodes.find((n) => n.def.kind === 'gate');
    const outcome = run.outcome;
    return h(
      'div.rv-expert',
      null,
      sectionOf('Steps', table(nodeCols, run.nodes, undefined, (n) => n.def.id)),
      sectionOf('Scores', rows.length ? h('div.rv-scroll', null, table(boardCols, rows, undefined, (r) => r.id)) : h('p.rv-faint', null, 'Nothing scored yet.')),
      sectionOf('Findings', findings.length ? h('div.rv-findings', null, findings.map((f) => findingNode(f))) : h('p.rv-faint', null, 'None.')),
      sectionOf(
        'The gate',
        h('p', null, `${runStateLabel(outcome?.decision ?? run.state)}${outcome?.by ? ` · by ${outcome.by}` : ''}`),
        outcome?.reasons.length ? listOf(outcome.reasons) : h('p.rv-faint', null, 'No reasons given.'),
        outcome?.replan.length ? sectionOf('Replan', listOf(outcome.replan)) : null,
        run.rules.length ? h('dl.rv-rules', null, run.rules.map((r) => [h('dt', null, r.when), h('dd', null, r.then)]).flat()) : null,
        gate?.notes ? h('p.rv-faint', null, gate.notes) : null,
      ),
    );
  };

  const head = (): Node => {
    if (!detail) {
      return h('div.rv-verdict', null, h('p.rv-sentence', null, failed && !loaded ? 'Could not load the review.' : loaded ? 'No review yet.' : 'Loading the review…'));
    }
    return h(
      'div.rv-verdict',
      null,
      badge(runStateLabel(detail.state), runTone(detail.state)),
      h('p.rv-sentence', null, detail.verdict || runStateLabel(detail.state)),
      h('span.rv-faint', null, `Round ${detail.round} of ${detail.rounds}`),
      tabs([{ key: 'plain', label: 'Plain' }, { key: 'expert', label: 'Expert' }], mem.expert ? 'expert' : 'plain', (key) => {
        mem.expert = key === 'expert';
        draw();
      }),
    );
  };

  const runsList = (): Node | null => {
    if (list.length < 2) return null;
    return h(
      'div.rv-runs',
      null,
      h('span.rv-faint', null, 'Earlier runs'),
      list.map((r) =>
        h(
          'button.rv-run',
          { type: 'button', 'aria-pressed': String(r.run === detail?.run), onclick: () => choose(r.run) },
          `Round ${r.round}`,
          h('span.rv-run-state', null, runStateLabel(r.state)),
          h('code', null, r.run),
        ),
      ),
    );
  };

  const choose = (run: string): void => {
    if (mem.run === run) return;
    mem.run = run;
    mem.open = null;
    mem.sel = NONE;
    mem.lens = null;
    void refresh();
  };

  const draw = (): void => {
    if (!alive) return;
    replace(headEl, head());
    replace(runsEl, runsList());
    if (!detail || !tree || !index) {
      scene?.destroy();
      scene = null;
      sceneKey = '';
      replace(bodyEl, loaded && !failed ? empty('approval', 'No review yet', 'The review starts once the work is sealed into a round.') : h('p.rv-faint', null, failed ? 'The review did not load. It will try again.' : 'Loading the review…'));
      return;
    }
    if (mem.expert) {
      scene?.destroy();
      scene = null;
      sceneKey = '';
      replace(bodyEl, expertView(detail, index));
      return;
    }
    const key = `${detail.run}@${detail.updated_ms}`;
    if (key !== sceneKey || !scene) {
      buildPlain(detail, tree);
      sceneKey = key;
    }
    applySelection();
  };

  const refresh = async (): Promise<void> => {
    if (!alive || busy) return;
    busy = true;
    try {
      const page = await review.runs(taskId);
      if (!alive) return;
      list = page.runs;
      failed = false;
      const runId = mem.run ?? list[0]?.run ?? null;
      if (runId && mem.run === null) mem.run = runId;
      if (!runId) {
        detail = null;
        tree = null;
        index = null;
        views = [];
      } else {
        const summary = list.find((r) => r.run === runId);
        if (!detail || detail.run !== runId || (summary !== undefined && summary.updated_ms !== stamp)) {
          const [d, t] = await Promise.all([review.run(runId), review.tree(runId)]);
          if (!alive) return;
          detail = d;
          tree = t;
          index = indexTree(t.tree);
          views = intentViews(index);
          stamp = summary?.updated_ms ?? d.updated_ms;
        }
        if (detail && recipeFor !== detail.recipe) {
          recipeFor = detail.recipe;
          const at = detail.recipe.lastIndexOf('@');
          const version = at > 0 ? Number(detail.recipe.slice(at + 1)) : NaN;
          recipe = await review.recipe(at > 0 ? detail.recipe.slice(0, at) : detail.recipe, Number.isInteger(version) ? version : undefined).catch(() => null);
          if (!alive) return;
        }
      }
      loaded = true;
      if (detail && tree && index) loadedOf.set(taskId, { list, detail, tree, index, views, recipe, stamp });
      const key = JSON.stringify([list, detail, tree, recipe?.recipe.axes ?? null]);
      if (key !== lastKey) {
        lastKey = key;
        draw();
      }
    } catch {
      failed = true;
      if (!detail) draw();
    } finally {
      busy = false;
    }
  };

  const loop = (): void => {
    window.setTimeout(async () => {
      if (!alive) return;
      if (root.isConnected) detached = 0;
      else if (++detached > 2) {
        alive = false;
        scene?.destroy();
        return;
      }
      if (!document.hidden) await refresh();
      loop();
    }, POLL_MS);
  };

  if (cached) draw();
  void refresh();
  loop();
  return root;
}
