// The review of one task's change, for the owner: the review graph (the task in the middle, the four readings as a ring,
// the change's intents round it, an intent opened into its definitions, a definition into what it touches), the words
// in a side panel, and the review's steps in order below it. Expert: the step table, the score board, the findings and
// the gate's reasons. Markup and wording follow the prototype (shared/m-review.js). Polls the hub (the store does not
// carry review frames).

import '../styles/proto/review.css';
import { review, type ChangeTree, type RecipeDetail, type ReviewFinding, type ReviewNode, type ReviewRun, type ReviewRunDetail, type ReviewScore, type ReviewTree, type Touch, type TreeFile, type WordedTouch } from '../api-review';
import { agentName, roleLabel } from '../roles';
import { agents } from '../store';
import { badge, clip, empty, table, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import {
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
  lowestOf,
  nameOf,
  nodeTone,
  ownerTone,
  polar,
  READING_AXES,
  readingTone,
  RADIUS,
  rowX,
  runStateLabel,
  runTone,
  scoreRows,
  spread,
  wordedText,
  type IntentView,
  type NodeTone,

  type ScoreRow,
  type TreeIndex,
} from './review-model';

const POLL_MS = 4_000;
const SVG_NS = 'http://www.w3.org/2000/svg';
/** The centre of the graph's view (the view is 640 units square; the plain view shows its middle 480). */
const C = 320;

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
interface Loaded { list: ReviewRun[]; detail: ReviewRunDetail; tree: ReviewTree; recipe: RecipeDetail | null; stamp: number }
const loadedOf = new Map<string, Loaded>();

// ---- A definition as the graph reads it: a symbol, or a file (its path) ---------------------------------------------

interface Def { id: string; name: string; path: string; kind: string; scores: ReviewScore[]; file: TreeFile; touches: Touch[]; worded: WordedTouch[] }

function defsOf(tree: ChangeTree): Map<string, Def> {
  const defs = new Map<string, Def>();
  for (const f of tree.files) {
    defs.set(f.path, { id: f.path, name: f.path, path: f.path, kind: '', scores: f.scores, file: f, touches: [], worded: [] });
    for (const s of f.symbols) defs.set(s.id, { id: s.id, name: s.qualified, path: f.path, kind: s.kind, scores: s.scores, file: f, touches: s.touches, worded: s.worded_touches });
  }
  return defs;
}

/** The scores that bear on a definition: its own and the file's, on the given axes when there are some. */
const scoresOfDef = (d: Def, axes: readonly string[] | null): ReviewScore[] => [...d.scores, ...d.file.scores].filter((s) => !axes || axes.includes(s.axis));

interface TouchItem { text: string; changed: boolean; worded: boolean }

/** A definition's touches and its worded touches as one list, in the order the side panel names them. */
function touchItemsOf(defs: Map<string, Def>, d: Def): TouchItem[] {
  return [
    ...d.touches.map((t) => ({ text: `${t.how} ${defs.get(t.to)?.name ?? t.to}`, changed: t.changed, worded: false })),
    ...d.worded.map((w) => ({ text: wordedText(w), changed: false, worded: true })),
  ];
}

// ---- Small builders -----------------------------------------------------------------------------------------------

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

/** Makes a graphic element a button: a click or Enter/Space runs `fn` (the SVG dots are not buttons). */
function activable(g: SVGElement, label: string, fn: () => void): void {
  g.setAttribute('tabindex', '0');
  g.setAttribute('role', 'button');
  g.setAttribute('aria-label', label);
  g.addEventListener('click', fn);
  g.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      fn();
    }
  });
}

const f1 = (n: number): string => n.toFixed(1);

const runLabel = (state: string): string => runStateLabel(state);

/** A score as a span: `axis: level · word · basis`, with its evidence as the tooltip. */
function scoreLine(s: ReviewScore): HTMLElement {
  return h('span', { title: s.evidence }, `${axisName(s.axis)}: ${s.level} · ${levelWord(s.level)} · ${basisLabel(s.basis)}`);
}

function findingItem(f: ReviewFinding, label: string): HTMLElement {
  const major = f.severity === 'major' || f.severity === 'blocker';
  return h('div.rv-line', null, h('span.muted', null, `Round ${f.round}`), h('span', { class: major ? 'rv-major' : null }, f.severity), h('span', null, f.text), h('span.muted', null, label));
}

function listOf(items: (Node | string)[]): HTMLElement {
  return h('ul.rv-list', null, items.map((i) => h('li', null, i)));
}

/** The reasons, under a heading: for a round that did not pass, or for any round when `always` is set. */
function whyBlock(o: ReviewRunDetail['outcome'], always: boolean): HTMLElement | null {
  if (!o || !o.reasons.length) return null;
  if (!always && o.decision === 'pass') return null;
  return h('div', null, h('h4', null, 'Why'), listOf(o.reasons));
}

function cellNode(scores: readonly ReviewScore[], axis: string): HTMLElement {
  const s = cellOf(scores, axis);
  if (!s) return h('span.muted', null, '·');
  const basis = s.basis === 'measured' ? 'measured' : 'estimated';
  const tone = s.level > 0 ? 'better' : s.level === 0 ? 'neutral' : 'worse';
  return h(
    'span.rv-cell',
    { class: `tone-${tone}`, title: `${axisName(axis)}: ${levelWord(s.level)}, ${basis} (confidence ${Math.round(s.confidence * 100)}%). ${s.evidence}` },
    String(s.level),
    h('em', null, basis),
  );
}

const toneOfReading = (level: number | null): string => {
  if (level === null) return 'muted';
  return level > 0 ? 'ok' : level < 0 ? 'danger' : 'muted';
};

// ---- The panel ----------------------------------------------------------------------------------------------------

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

  const cached = loadedOf.get(taskId) ?? null;
  let alive = true;
  let busy = false;
  let detached = 0;
  let loaded = cached !== null;
  let failed = false;
  let list: ReviewRun[] = cached?.list ?? [];
  let detail: ReviewRunDetail | null = cached?.detail ?? null;
  let tree: ReviewTree | null = cached?.tree ?? null;
  let recipe: RecipeDetail | null = cached?.recipe ?? null;
  let stamp = cached?.stamp ?? -1;
  let recipeFor = cached?.detail.recipe ?? '';
  let lastKey = '';
  let model: { index: TreeIndex; defs: Map<string, Def>; views: IntentView[]; all: ReviewFinding[] } | null = null;

  const isSel = (kind: Sel['kind'], id: string): boolean => {
    const s = mem.sel;
    if (s.kind !== kind) return false;
    if (s.kind === 'intent') return s.key === id;
    if (s.kind === 'def') return s.id === id;
    if (s.kind === 'node') return s.id === id;
    if (s.kind === 'reading') return s.name === id;
    return false;
  };

  const clearAll = (): void => {
    mem.open = null;
    mem.lens = null;
    mem.sel = NONE;
  };

  /** Each of these is one tap in the panel; each one draws the panel again. */
  const act = (change: () => void): void => {
    change();
    draw();
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

  const toggleIntent = (key: string): void => {
    if (mem.open === key) clearAll();
    else {
      mem.open = key;
      mem.sel = { kind: 'intent', key };
    }
  };

  const toggleDef = (id: string): void => {
    if (isSel('def', id)) mem.sel = mem.open !== null ? { kind: 'intent', key: mem.open } : NONE;
    else mem.sel = { kind: 'def', id };
  };

  const toggleTouch = (from: string, index: number): void => {
    const same = mem.sel.kind === 'touch' && mem.sel.from === from && mem.sel.index === index;
    mem.sel = same ? { kind: 'def', id: from } : { kind: 'touch', from, index };
  };

  const toggleNode = (id: string): void => {
    mem.sel = isSel('node', id) ? NONE : { kind: 'node', id };
  };

  /** The lens's axes, or null when no reading colours the intents. */
  const lensAxes = (): readonly string[] | null => (mem.lens ? READING_AXES[mem.lens] ?? [] : null);

  /** The words for the side panel: what is selected, its scores, its findings. */
  const sideContent = (run: ReviewRunDetail, t: ReviewTree, mdl: NonNullable<typeof model>): (Node | string | null)[] => {
    const s = mem.sel;
    const { index, defs, views, all } = mdl;
    const o = run.outcome;
    const none = (): (Node | string | null)[] => {
      const files = t.tree.files.length;
      return [
        h('h3', null, 'The change'),
        h('p.muted', null, `${files} ${files === 1 ? 'file' : 'files'} · tap a dot to open it`),
        ...t.readings.map((r) => h('div.rv-line', null, h('span.rv-sd', { class: `t-${toneOfReading(r.level)}` }), h('span', null, r.name), h('span.muted', null, levelWord(r.level)))),
        whyBlock(o, false),
      ];
    };
    if (s.kind === 'none') return none();
    if (s.kind === 'reading') {
      const r = t.readings.find((x) => x.name === s.name);
      const axes = READING_AXES[s.name] ?? [];
      const low = index.tree.files
        .flatMap((f) => [...f.scores, ...f.symbols.flatMap((sym) => sym.scores)])
        .filter((x) => axes.includes(x.axis))
        .sort((a, b) => a.level - b.level)
        .slice(0, 10);
      return [
        h('h3', null, s.name),
        h('p', null, levelWord(r ? r.level : null)),
        h('p.muted', null, mem.lens === s.name ? 'The intents are coloured by this. Tap the segment again to clear it.' : 'Tap the segment to colour the intents by it.'),
        ...(low.length
          ? low.map((x) => h('div.rv-line', null, h('span', null, labelOf(defs, x.symbol)), h('span.muted', null, '·'), scoreLine(x)))
          : [h('p.muted', null, 'Nothing measured on it yet.')]),
      ];
    }
    if (s.kind === 'intent') {
      const it = views.find((v) => v.key === s.key);
      if (!it) return none();
      const ds = it.items.map((id) => defs.get(id)).filter((d): d is Def => d !== undefined);
      const cnt = it.items.length;
      const fs = all.filter((f) => it.items.includes(f.symbol));
      return [
        h('h3', null, it.title),
        h('p.muted', null, `${kindLabel(it.kind)} · ${cnt} ${cnt === 1 ? 'definition' : 'definitions'}`),
        it.summary ? h('p', null, it.summary) : null,
        ...(ds.length
          ? ds.map((d) => h('button.rv-link', { type: 'button', onclick: () => act(() => toggleDef(d.id)) }, d.name))
          : [h('p.muted', null, 'Nothing in it.')]),
        fs.length ? h('h4', null, 'Findings') : null,
        ...fs.map((f) => findingItem(f, labelOf(defs, f.symbol))),
      ];
    }
    if (s.kind === 'def') {
      const d = defs.get(s.id);
      if (!d) return none();
      const items = touchItemsOf(defs, d);
      const fs = all.filter((f) => f.symbol === d.id);
      return [
        h('h3', null, d.name),
        h('p.muted', null, d.kind ? `${d.path} · ${kindLabel(d.kind)}` : d.path),
        h('h4', null, 'Scores'),
        ...(d.scores.length ? d.scores.map((x) => h('div.rv-line', null, scoreLine(x))) : [h('p.muted', null, 'Not scored yet.')]),
        fs.length ? h('h4', null, 'Findings') : null,
        ...fs.map((f) => findingItem(f, labelOf(defs, f.symbol))),
        h('h4', null, 'Touches'),
        ...(items.length
          ? items.map((x, j) => h('button.rv-link', { type: 'button', class: x.changed && !x.worded ? 'changed' : null, onclick: () => act(() => toggleTouch(d.id, j)) }, x.text))
          : [h('p.muted', null, 'Touches nothing else.')]),
      ];
    }
    if (s.kind === 'touch') {
      const d = defs.get(s.from);
      const t1 = d ? touchItemsOf(defs, d)[s.index] : undefined;
      if (!d || !t1) return none();
      return [
        h('h3', null, d.name),
        h('p', null, t1.text),
        t1.changed ? h('p.muted', null, 'Changed in this change.') : null,
        h('button.btn.ghost.sm', { type: 'button', onclick: () => act(() => toggleDef(d.id)) }, `Back to ${d.name}`),
      ];
    }
    // A step of the flow, or the owner's ruling.
    if (s.id === 'owner') {
      const dec = o ? o.decision : null;
      return [
        h('h3', null, 'Your call'),
        h('p', null, dec === 'owner' ? 'The gate sent it to you.' : dec ? `Ruled ${runLabel(dec).toLowerCase()}.` : 'Waiting for the gate.'),
        whyBlock(o, true),
      ];
    }
    const n = run.nodes.find((x) => x.def.id === s.id);
    if (!n) return none();
    const who = n.def.role ? `${roleLabel(n.def.role)} · ${assigneeName(n.assignee)}` : assigneeName(n.assignee);
    return [
      h('h3', null, n.def.id),
      h('p.muted', null, who),
      h('p', null, runLabel(n.state)),
      n.notes ? h('p.muted', null, n.notes) : null,
      n.scores.length ? h('h4', null, 'Scores') : null,
      ...n.scores.map((x) => h('div.rv-line', null, scoreLine(x))),
      n.findings.length ? h('h4', null, 'Findings') : null,
      ...n.findings.map((f) => findingItem(f, labelOf(defs, f.symbol))),
    ];
  };

  /** The radial graph: the readings' ring, the intents, an open intent's definitions, a definition's touches, the task. */
  const graphSvg = (run: ReviewRunDetail, t: ReviewTree, mdl: NonNullable<typeof model>): SVGSVGElement => {
    const { defs, views } = mdl;
    const openIdx = mem.open === null ? -1 : views.findIndex((v) => v.key === mem.open);
    const openView = openIdx >= 0 ? views[openIdx] ?? null : null;
    const lens = lensAxes();
    const svg = svgEl<SVGSVGElement>('svg', { viewBox: openView ? '0 0 640 640' : '80 80 480 480', role: 'img', 'aria-label': 'The review graph' }, 'rv-svg');
    const backdrop = svgEl('rect', { x: 0, y: 0, width: 640, height: 640, fill: 'transparent' });
    backdrop.addEventListener('click', () => act(clearAll));
    svg.append(backdrop);

    const readings = t.readings;
    readings.forEach((r, i) => {
      const { from, to } = arcSpan(i, readings.length);
      const on = mem.lens === r.name ? ' on' : '';
      const arc = svgEl<SVGPathElement>('path', { d: arcPath(C, C, RADIUS.ring, from, to), 'data-v': r.name }, `rv-arc ${readingTone(r.level)}${on}`);
      titled(arc, `${r.name}: ${r.level === null ? 'not measured' : levelWord(r.level)}`);
      activable(arc, r.name, () => act(() => toggleLens(r.name)));
      svg.append(arc);
      const [lx, ly] = polar(C, C, 196, (from + to) / 2);
      const label = svgEl('text', { x: f1(lx), y: f1(ly), 'text-anchor': 'middle', 'dominant-baseline': 'middle' }, 'rv-arc-l');
      label.textContent = r.name;
      svg.append(label);
    });

    const n = views.length;
    const ip = views.map((_, i) => polar(C, C, RADIUS.intent, intentAngle(i, n)));
    views.forEach((v, i) => {
      const [x, y] = ip[i]!;
      const cnt = v.items.length;
      const tone = intentTone(lowestOf(v.items.flatMap((id) => (defs.get(id) ? scoresOfDef(defs.get(id)!, lens) : []))));
      const r = intentRadius(cnt);
      const title = v.title;
      svg.append(svgEl('line', { x1: C, y1: C, x2: f1(x), y2: f1(y) }, 'rv-spoke'));
      const g = svgEl<SVGGElement>('g', { 'data-id': i }, `rv-i${mem.open === v.key ? ' open' : ''}${isSel('intent', v.key) ? ' sel' : ''}${v.orphan ? ' orphan' : ''} t-${tone}`);
      titled(g, `${title}: ${cnt} ${cnt === 1 ? 'definition' : 'definitions'}`);
      g.append(svgEl('circle', { cx: f1(x), cy: f1(y), r: 13 }, 'rv-hit'), svgEl('circle', { cx: f1(x), cy: f1(y), r: f1(r) }, 'rv-dot'));
      const label = svgEl('text', { x: f1(x), y: f1(y + r + 16), 'text-anchor': 'middle', 'dominant-baseline': 'hanging' }, 'rv-i-l');
      label.textContent = clip(title, 18);
      g.append(label);
      activable(g, title, () => act(() => toggleIntent(v.key)));
      svg.append(g);
    });

    const dpos = new Map<string, { x: number; y: number; ang: number }>();
    if (openView && openIdx >= 0) {
      const [ix, iy] = ip[openIdx]!;
      const base = intentAngle(openIdx, n);
      const ids = openView.items.filter((id) => defs.has(id));
      const angles = spread(base, ids.length, Math.min(120, Math.max(0, (ids.length - 1) * 18)));
      ids.forEach((id, j) => {
        const d = defs.get(id)!;
        const ang = angles[j] ?? base;
        const [x, y] = polar(C, C, RADIUS.def, ang);
        dpos.set(id, { x, y, ang });
        const sel = isSel('def', id) || (mem.sel.kind === 'touch' && mem.sel.from === id);
        svg.append(svgEl('line', { x1: f1(ix), y1: f1(iy), x2: f1(x), y2: f1(y) }, 'rv-dlink'));
        const g = svgEl<SVGGElement>('g', { 'data-id': id }, `rv-d-g${sel ? ' sel' : ''} t-${intentTone(lowestOf(scoresOfDef(d, lens)))}`);
        titled(g, d.name);
        g.append(svgEl('circle', { cx: f1(x), cy: f1(y), r: 14 }, 'rv-hit'), svgEl('circle', { cx: f1(x), cy: f1(y), r: 6.5 }, 'rv-dot'));
        activable(g, d.name, () => act(() => toggleDef(id)));
        svg.append(g);
      });
    }

    const sd = mem.sel.kind === 'def' || mem.sel.kind === 'touch' ? defs.get(mem.sel.kind === 'def' ? mem.sel.id : mem.sel.from) : undefined;
    const base = sd ? dpos.get(sd.id) : undefined;
    if (sd && base) {
      const items = touchItemsOf(defs, sd);
      const angles = spread(base.ang, items.length, Math.min(90, Math.max(0, (items.length - 1) * 12)));
      items.forEach((t1, j) => {
        const [x, y] = polar(C, C, RADIUS.touch, angles[j] ?? base.ang);
        const sel = mem.sel.kind === 'touch' && mem.sel.from === sd.id && mem.sel.index === j;
        svg.append(svgEl('line', { x1: f1(base.x), y1: f1(base.y), x2: f1(x), y2: f1(y) }, `rv-tlink${t1.worded ? ' worded' : ''}`));
        const g = svgEl<SVGGElement>('g', { 'data-j': j }, `rv-t-g${t1.changed ? ' changed' : ''}${t1.worded ? ' worded' : ''}${sel ? ' sel' : ''} t-none`);
        titled(g, t1.text);
        g.append(svgEl('circle', { cx: f1(x), cy: f1(y), r: 9 }, 'rv-hit'), svgEl('circle', { cx: f1(x), cy: f1(y), r: 5 }, 'rv-dot'));
        activable(g, t1.text, () => act(() => toggleTouch(sd.id, j)));
        svg.append(g);
      });
    }

    const centre = svgEl<SVGGElement>('g', {}, `rv-centre t-${runTone(run.state)}`);
    titled(centre, `${run.task}: ${runLabel(run.state)}`);
    centre.append(svgEl('circle', { cx: C, cy: C, r: RADIUS.task }, 'rv-dot'));
    const name = svgEl('text', { x: C, y: C, 'text-anchor': 'middle', 'dominant-baseline': 'middle' });
    name.textContent = run.task;
    centre.append(name);
    activable(centre, run.task, () => act(clearAll));
    svg.append(centre);
    return svg;
  };

  /** The review's steps in order, the gate, then the owner: one dot each, a pulse on the running one. */
  const flowSvg = (run: ReviewRunDetail): SVGSVGElement => {
    const dec = run.outcome ? run.outcome.decision : null;
    const steps = run.nodes.filter((x) => x.def.kind !== 'gate');
    const gates = run.nodes.filter((x) => x.def.kind === 'gate');
    const items: { id: string; tone: NodeTone; gate: boolean; label: string }[] = [
      ...steps.map((x) => ({ id: x.def.id, tone: nodeTone(x.state), gate: false, label: `${x.def.id}: ${runLabel(x.state)}` })),
      ...gates.map((x) => ({ id: x.def.id, tone: dec ? ownerTone(dec) : nodeTone(x.state), gate: true, label: `${x.def.id}: ${runLabel(x.state)}` })),
      {
        id: 'owner',
        tone: dec === 'owner' ? 'owner' : run.outcome && run.outcome.by === 'owner' ? ownerTone(dec) : 'wait',
        gate: true,
        label: `owner: ${dec ? runLabel(dec) : 'Waiting'}`,
      },
    ];
    const N = items.length;
    const xs = items.map((_, i) => rowX(i, N, 40, 600));
    const svg = svgEl<SVGSVGElement>('svg', { viewBox: '0 0 640 80', role: 'img', 'aria-label': 'The review steps, in order' }, 'rv-svg-flow');
    xs.slice(1).forEach((x, i) => svg.append(svgEl('line', { x1: f1(xs[i] ?? 0), y1: 40, x2: f1(x), y2: 40 }, 'rv-fl')));
    items.forEach((it, i) => {
      const x = xs[i] ?? 0;
      const sel = mem.sel.kind === 'node' && mem.sel.id === it.id;
      const pulse = it.tone === 'run' || it.tone === 'owner' ? ' pulse' : '';
      const g = svgEl<SVGGElement>('g', {}, `rv-fd t-${it.tone}${sel ? ' sel' : ''}${pulse}`);
      titled(g, it.label);
      g.append(svgEl('circle', { cx: f1(x), cy: 40, r: 8 }, 'rv-hit'), svgEl('circle', { cx: f1(x), cy: 40, r: it.gate ? 10 : 7 }, 'rv-dot'));
      activable(g, it.label, () => act(() => toggleNode(it.id)));
      svg.append(g);
    });
    return svg;
  };

  const expertNode = (run: ReviewRunDetail, mdl: NonNullable<typeof model>): HTMLElement => {
    const nodeCols: Column<ReviewNode>[] = [
      { label: 'Step', cell: (x) => h('code', null, x.def.id) },
      { label: 'Kind', cell: (x) => x.def.kind },
      { label: 'Reviewer', cell: (x) => (x.def.role ? roleLabel(x.def.role) : '—') },
      { label: 'Assignee', cell: (x) => assigneeName(x.assignee) },
      { label: 'State', cell: (x) => runLabel(x.state) },
      { label: 'Notes', cell: (x) => x.notes || '' },
    ];
    const rows: ScoreRow[] = scoreRows(mdl.index);
    const axes = recipe && recipe.recipe.axes.length ? recipe.recipe.axes : [...new Set(rows.flatMap((r) => r.scores.map((s) => s.axis)))];
    const boardCols: Column<ScoreRow>[] = [
      { label: 'Definition', cell: (r) => h('span', { title: r.id }, r.name) },
      ...axes.map((a) => ({ label: axisName(a), num: true, cell: (r: ScoreRow) => cellNode(r.scores, a) })),
    ];
    const seen = new Set<string>();
    const findings = mdl.all
      .filter((f) => {
        const k = `${f.round}|${f.node}|${f.symbol}|${f.text}`;
        if (seen.has(k)) return false;
        seen.add(k);
        return true;
      })
      .sort((a, b) => b.round - a.round);
    const dec = run.outcome ? run.outcome.decision : null;
    const outcome = run.outcome;
    const gate = run.nodes.find((x) => x.def.kind === 'gate');
    return h(
      'div.rv-expert',
      null,
      h('section.rv-block', null, h('h3', null, 'Steps'), h('div.tbl-wrap', null, table(nodeCols, run.nodes, undefined, (x) => x.def.id))),
      h(
        'section.rv-block',
        null,
        h('h3', null, 'Scores'),
        rows.length ? h('div.tbl-wrap', null, table(boardCols, rows, undefined, (r) => r.id)) : h('p.muted', null, 'Nothing scored yet.'),
      ),
      h('section.rv-block', null, h('h3', null, 'Findings'), ...(findings.length ? findings.map((f) => findingItem(f, labelOf(mdl.defs, f.symbol))) : [h('p.muted', null, 'None.')])),
      h(
        'section.rv-block',
        null,
        h('h3', null, 'The gate'),
        h('p', null, `${runLabel(dec ?? run.state)}${outcome?.by ? ` · by ${outcome.by}` : ''}`),
        outcome && outcome.reasons.length ? listOf(outcome.reasons) : h('p.muted', null, 'No reasons given.'),
        outcome && outcome.replan.length ? h('div', null, h('h4', null, 'Replan'), listOf(outcome.replan)) : null,
        h('h4', null, 'Rules'),
        h('dl.rv-rules', null, run.rules.flatMap((r) => [h('dt', null, r.when), h('dd', null, r.then)])),
        gate && gate.notes ? h('p.muted', null, gate.notes) : null,
      ),
    );
  };

  const headNode = (run: ReviewRunDetail): HTMLElement =>
    h(
      'div.rv-head',
      null,
      h(
        'div.rv-head-l',
        null,
        badge(runLabel(run.state), runTone(run.state), true),
        h('p.rv-verdict', null, run.verdict || runLabel(run.state)),
        h('span.rv-round', null, `Round ${run.round} of ${run.rounds}`),
      ),
      h(
        'div.rv-tabs',
        null,
        h('button.btn.ghost.sm', { type: 'button', 'aria-pressed': String(!mem.expert), onclick: () => act(() => (mem.expert = false)) }, 'Plain'),
        h('button.btn.ghost.sm', { type: 'button', 'aria-pressed': String(mem.expert), onclick: () => act(() => (mem.expert = true)) }, 'Expert'),
      ),
    );

  const runsNode = (): HTMLElement | null => {
    if (list.length < 2) return null;
    return h(
      'div.rv-runs',
      null,
      h('span.muted', null, 'Earlier runs'),
      list.map((r) =>
        h(
          'button.btn.ghost.sm.rv-run',
          { type: 'button', 'aria-pressed': String(r.run === mem.run), onclick: () => choose(r.run) },
          h('span', null, `Round ${r.round}`),
          ' ',
          h('span.rv-rs', null, runLabel(r.state)),
          ' ',
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

  /** Draws the whole panel for the memory: the head, the body (plain or expert) and the earlier runs. */
  const draw = (): void => {
    if (!alive) return;
    if (!detail || !tree) {
      replace(root, loaded && !failed ? empty('approval', 'No review yet', 'The review starts once the work is sealed into a round.') : h('p.muted', null, failed ? 'The review did not load. It will try again.' : 'Loading the review…'));
      return;
    }
    if (!model) return;
    const run = detail;
    const t = tree;
    const mdl = model;
    const body = mem.expert
      ? expertNode(run, mdl)
      : h(
          'div.rv-stage',
          null,
          h('div.rv-canvas', null, graphSvg(run, t, mdl)),
          h('aside.rv-side', null, ...sideContent(run, t, mdl)),
        );
    const plain = mem.expert ? null : h('div.rv-flowwrap', null, h('p.muted', null, 'Steps, in order'), flowSvg(run));
    replace(root, headNode(run), h('div.rv-body', null, body, plain), runsNode());
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
        model = null;
      } else {
        const summary = list.find((r) => r.run === runId);
        if (!detail || detail.run !== runId || (summary !== undefined && summary.updated_ms !== stamp)) {
          const [d, t] = await Promise.all([review.run(runId), review.tree(runId)]);
          if (!alive) return;
          detail = d;
          tree = t;
          const index = indexTree(t.tree);
          model = { index, defs: defsOf(t.tree), views: intentViews(index), all: d.nodes.flatMap((x) => x.findings) };
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
      if (detail && tree) loadedOf.set(taskId, { list, detail, tree, recipe, stamp });
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
        return;
      }
      if (!document.hidden) await refresh();
      loop();
    }, POLL_MS);
  };

  // A cached run draws at once; a new one shows "Loading the review…" until the hub answers.
  if (cached && detail && tree) {
    const index = indexTree(tree.tree);
    model = { index, defs: defsOf(tree.tree), views: intentViews(index), all: detail.nodes.flatMap((x) => x.findings) };
  }
  draw();
  void refresh();
  loop();
  return root;
}

// ---- Words the panel shares ---------------------------------------------------------------------------------------

/** A definition's name for people (its qualified name, or its path), or the id when the tree does not have it. */
function labelOf(defs: Map<string, Def>, id: string): string {
  return defs.get(id)?.name ?? nameOf(id);
}

/** The agent's name for an assignee id, or `agent N` while the roster does not have it. */
function assigneeName(cid: number | null): string {
  if (cid === null) return '—';
  const a = agents.get().find((x) => x.cid === cid);
  return a ? agentName(a) : `agent ${cid}`;
}

