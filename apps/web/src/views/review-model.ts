// The review graph's pure helpers: the readings' tones, an intent's lowest score, the layout angles, the recipe layers and
// the version bump. No DOM and no imports at run time, so the tests load this file on its own.

import type { ChangeTree, ReviewFinding, ReviewScore, Touch, TreeFile, TreeSymbol, WordedTouch } from '../api-review';

// ---- Readings and scores ------------------------------------------------------------------------------------------

export type ReadingTone = 'better' | 'neutral' | 'worse' | 'none';

/** A reading's colour: above zero better, below zero worse, zero neutral, none when it was not measured. */
export function readingTone(level: number | null | undefined): ReadingTone {
  if (level === null || level === undefined || !Number.isFinite(level)) return 'none';
  if (level > 0) return 'better';
  if (level < 0) return 'worse';
  return 'neutral';
}

/** The plain word for a level, the one the side panel shows beside the number. */
export function levelWord(level: number | null | undefined): string {
  if (level === null || level === undefined) return 'not measured';
  if (level >= 3) return 'much better';
  if (level >= 1) return 'better';
  if (level === 0) return 'neutral';
  if (level >= -2) return 'worse';
  return 'much worse';
}

/** A score at or below this is a watch: the intent or definition needs a look. */
export const WATCH_AT = -1;
/** A score at or below this needs the owner's hand. */
export const NEEDS_AT = -3;

export type IntentTone = 'better' | 'watch' | 'needs' | 'none';

/** An intent's (or a definition's) colour, from its lowest score. Nothing scored: none. */
export function intentTone(lowest: number | null | undefined): IntentTone {
  if (lowest === null || lowest === undefined) return 'none';
  if (lowest <= NEEDS_AT) return 'needs';
  if (lowest <= WATCH_AT) return 'watch';
  return 'better';
}

/** The lowest level among `scores`, or null when there are none. */
export function lowestOf(scores: readonly { level: number }[]): number | null {
  let low: number | null = null;
  for (const s of scores) if (low === null || s.level < low) low = s.level;
  return low;
}

/** The axes behind each reading (the hub's grouping: function, resources, risk, quality). Other axes count for none. */
export const READING_AXES: Record<string, readonly string[]> = {
  'what it does': ['fea', 'ux', 'val', 'con'],
  'cost to run': ['mem', 'cpu', 'prf', 'cost'],
  risk: ['sec', 'cmp', 'div', 'leg'],
  'code health': ['tst', 'cln', 'dbt', 'mnt'],
};

const AXIS_NAMES: Record<string, string> = {
  fea: 'Feature', ux: 'UI/UX', val: 'Value', con: 'Contract', mem: 'Memory', cpu: 'Compute', prf: 'Performance',
  sec: 'Security', cmp: 'Compatibility', div: 'Divergence', leg: 'Legacy', tst: 'Tests', cln: 'Clean code',
  dbt: 'Tech debt', mnt: 'Maintenance', acc: 'Accuracy', src: 'Sourcing', cov: 'Coverage', cost: 'Cost',
};

/** An axis's name for people; an unknown axis shows its id. */
export const axisName = (axis: string): string => AXIS_NAMES[axis] ?? axis;

// ---- Definitions, files and intents -------------------------------------------------------------------------------

/** The file a definition id belongs to: the part before `::` (a file's own id is its path). */
export function pathOf(id: string): string {
  const i = id.indexOf('::');
  return i < 0 ? id : id.slice(0, i);
}

/** A definition's name for people: the part after `::`, or the path for a file. */
export function nameOf(id: string): string {
  const i = id.indexOf('::');
  return i < 0 ? id : id.slice(i + 2);
}

export interface TreeIndex {
  tree: ChangeTree;
  symbols: Map<string, TreeSymbol>;
  files: Map<string, TreeFile>;
}

export function indexTree(tree: ChangeTree): TreeIndex {
  const symbols = new Map<string, TreeSymbol>();
  const files = new Map<string, TreeFile>();
  for (const file of tree.files) {
    files.set(file.path, file);
    for (const s of file.symbols) symbols.set(s.id, s);
  }
  return { tree, symbols, files };
}

/** The scores that bear on a definition: its own, and the file's (a measurement of the file as a whole). Optionally only some axes. */
export function scoresOf(idx: TreeIndex, id: string, axes?: readonly string[] | null): ReviewScore[] {
  const own = idx.symbols.get(id)?.scores ?? [];
  const file = idx.files.get(pathOf(id));
  const whole = file ? file.scores.filter((s) => s.symbol === file.path) : [];
  const all = [...own, ...whole];
  return axes ? all.filter((s) => axes.includes(s.axis)) : all;
}

/** The lowest score across some definitions (optionally only on some axes): the colour of an intent. */
export function lowestFor(idx: TreeIndex, ids: readonly string[], axes?: readonly string[] | null): number | null {
  return lowestOf(ids.flatMap((id) => scoresOf(idx, id, axes)));
}

export interface IntentView { key: string; title: string; kind: string; summary: string; items: string[]; orphan: boolean }

/** The intents as the view draws them, then one group for what no intent covers ("Not in an intent"), when there is any. */
export function intentViews(idx: TreeIndex): IntentView[] {
  const views: IntentView[] = idx.tree.intents.map((it, i) => ({
    key: `i${i}`,
    title: it.title || 'Untitled intent',
    kind: it.kind,
    summary: it.summary,
    items: [...new Set(it.items)],
    orphan: false,
  }));
  const covered = new Set(views.flatMap((v) => v.items));
  const rest: string[] = [];
  const seen = new Set<string>();
  const take = (id: string): void => {
    if (covered.has(id) || seen.has(id)) return;
    seen.add(id);
    rest.push(id);
  };
  for (const file of idx.tree.files) for (const s of file.symbols) take(s.id);
  for (const u of idx.tree.unsorted) take(u);
  if (rest.length) views.push({ key: 'orphan', title: 'Not in an intent', kind: '', summary: '', items: rest, orphan: true });
  return views;
}

/** The findings a set of definitions carries, the ones the side panel lists. */
export function findingsFor(idx: TreeIndex, ids: readonly string[]): ReviewFinding[] {
  return ids.flatMap((id) => idx.symbols.get(id)?.findings ?? []);
}

/** Every finding of the change, once each (a finding may be listed under its file and its definition). */
export function allFindings(idx: TreeIndex): ReviewFinding[] {
  const out: ReviewFinding[] = [];
  const seen = new Set<string>();
  const add = (f: ReviewFinding): void => {
    const key = `${f.round}|${f.node}|${f.symbol}|${f.text}`;
    if (seen.has(key)) return;
    seen.add(key);
    out.push(f);
  };
  for (const file of idx.tree.files) for (const s of file.symbols) s.findings.forEach(add);
  return out;
}

/** What a definition touches, as its touches and its worded touches. */
export function touchesOf(idx: TreeIndex, id: string): { touches: Touch[]; worded: WordedTouch[] } {
  const s = idx.symbols.get(id);
  return { touches: s?.touches ?? [], worded: s?.worded_touches ?? [] };
}

/** A worded touch as text: the string itself, or its parts. */
export function wordedText(w: WordedTouch): string {
  if (typeof w === 'string') return w;
  return [w.how, w.to ?? w.text].filter((x): x is string => !!x).join(' ');
}

export interface ScoreRow { id: string; name: string; path: string; scores: ReviewScore[] }

/** Rows of the score board: each definition (or file) with a score, in file order. */
export function scoreRows(idx: TreeIndex): ScoreRow[] {
  const groups = new Map<string, ReviewScore[]>();
  const add = (s: ReviewScore): void => {
    const list = groups.get(s.symbol);
    if (list) list.push(s);
    else groups.set(s.symbol, [s]);
  };
  for (const file of idx.tree.files) {
    file.scores.forEach(add);
    for (const s of file.symbols) s.scores.forEach(add);
  }
  const rows: ScoreRow[] = [];
  for (const [id, scores] of groups) rows.push({ id, name: nameOf(id), path: pathOf(id), scores });
  return rows;
}

/** The score a cell shows for one axis: the lowest of that axis's scores on the definition, or null. */
export function cellOf(scores: readonly ReviewScore[], axis: string): ReviewScore | null {
  let worst: ReviewScore | null = null;
  for (const s of scores) if (s.axis === axis && (worst === null || s.level < worst.level)) worst = s;
  return worst;
}

// ---- Run states --------------------------------------------------------------------------------------------------

export type RunTone = 'ok' | 'warn' | 'accent' | 'danger' | 'info' | 'muted';

/** A run's state as people read it. */
export function runStateLabel(state: string): string {
  switch (state) {
    case 'passed':
    case 'pass':
      return 'Passed';
    case 'rework':
      return 'Sent back';
    case 'owner':
      return 'Your call';
    case 'failed':
    case 'fail':
      return 'Failed';
    case 'running':
    case 'working':
      return 'Running';
    case 'pending':
    case 'waiting':
      return 'Waiting';
    default:
      return state ? state.replace(/_/g, ' ').replace(/^./, (c) => c.toUpperCase()) : 'Unknown';
  }
}

export function runTone(state: string): RunTone {
  switch (state) {
    case 'passed':
    case 'pass':
      return 'ok';
    case 'rework':
      return 'warn';
    case 'owner':
      return 'accent';
    case 'failed':
    case 'fail':
      return 'danger';
    case 'running':
    case 'working':
      return 'info';
    default:
      return 'muted';
  }
}

export type NodeTone = 'ok' | 'fail' | 'run' | 'rework' | 'owner' | 'wait';

/** A step's state as a dot's tone in the flow. */
export function nodeTone(state: string): NodeTone {
  switch (state) {
    case 'passed':
    case 'pass':
    case 'done':
    case 'ok':
      return 'ok';
    case 'failed':
    case 'fail':
      return 'fail';
    case 'running':
    case 'working':
    case 'started':
    case 'active':
      return 'run';
    case 'rework':
      return 'rework';
    case 'owner':
      return 'owner';
    default:
      return 'wait';
  }
}

/** The gate's decision as a flow tone: where the owner's ruling sits. */
export function ownerTone(decision: string | null | undefined): NodeTone {
  if (decision === 'owner') return 'owner';
  if (decision === 'pass') return 'ok';
  if (decision === 'rework') return 'rework';
  if (decision === 'fail') return 'fail';
  return 'wait';
}

// ---- Recipes -----------------------------------------------------------------------------------------------------

/** Each step's layer: 0 for a step that waits on nothing, else one more than the deepest step it waits on. */
export function layerNodes(nodes: readonly { id: string; after: readonly string[] }[]): Map<string, number> {
  const byId = new Map(nodes.map((n) => [n.id, n] as const));
  const layer = new Map<string, number>();
  const visiting = new Set<string>();
  const depth = (id: string): number => {
    const known = layer.get(id);
    if (known !== undefined) return known;
    const node = byId.get(id);
    if (!node || visiting.has(id)) return 0;
    visiting.add(id);
    let d = 0;
    for (const p of node.after) if (byId.has(p)) d = Math.max(d, depth(p) + 1);
    visiting.delete(id);
    layer.set(id, d);
    return d;
  };
  for (const n of nodes) depth(n.id);
  return layer;
}

/** The recipe's version as its TOML states it (the top-level `version`), or null. */
export function versionOf(toml: string): number | null {
  for (const line of toml.split('\n')) {
    if (/^\s*\[/.test(line)) break;
    const m = /^\s*version\s*=\s*(\d+)\b/.exec(line);
    if (m) return Number(m[1]);
  }
  return null;
}

/**
 * The TOML with its version moved on: when its `version` is still `current` (the owner did not change it), it becomes
 * `current + 1`; a version the owner typed stays as typed. With no version line, one is added at the top.
 */
export function bumpVersion(toml: string, current: number): string {
  const lines = toml.split('\n');
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i] ?? '';
    if (/^\s*\[/.test(line)) break;
    const m = /^(\s*version\s*=\s*)(\d+)(.*)$/.exec(line);
    if (m) {
      if (Number(m[2]) === current) lines[i] = `${m[1]}${current + 1}${m[3]}`;
      return lines.join('\n');
    }
  }
  return [`version = ${current + 1}`, ...lines].join('\n');
}

// ---- Layout ------------------------------------------------------------------------------------------------------

/** The radii of the radial graph, in view units. */
export const RADIUS = { task: 26, intent: 112, ring: 168, def: 236, touch: 296 } as const;

/** A point on a circle; angles run clockwise from straight up. */
export function polar(cx: number, cy: number, r: number, deg: number): [number, number] {
  const a = (deg * Math.PI) / 180;
  return [cx + r * Math.sin(a), cy - r * Math.cos(a)];
}

/** `count` angles spread evenly over `span` degrees, centred on `centre`. */
export function spread(centre: number, count: number, span: number): number[] {
  if (count <= 0) return [];
  if (count === 1) return [centre];
  const step = span / (count - 1);
  return Array.from({ length: count }, (_, i) => centre - span / 2 + i * step);
}

/** The angle of the i-th of n intents: evenly round the circle, each one half a step off the top. */
export function intentAngle(i: number, n: number): number {
  return ((i + 0.5) * 360) / Math.max(1, n);
}

/** The arc of the i-th of n readings: a quarter of the ring each, with a gap either side. */
export function arcSpan(i: number, n: number, gap = 6): { from: number; to: number } {
  const step = 360 / Math.max(1, n);
  return { from: i * step + gap / 2, to: (i + 1) * step - gap / 2 };
}

/** An SVG path for the arc of a circle from `from` to `to` degrees. */
export function arcPath(cx: number, cy: number, r: number, from: number, to: number): string {
  const [x0, y0] = polar(cx, cy, r, from);
  const [x1, y1] = polar(cx, cy, r, to);
  const large = to - from > 180 ? 1 : 0;
  return `M ${x0.toFixed(2)} ${y0.toFixed(2)} A ${r} ${r} 0 ${large} 1 ${x1.toFixed(2)} ${y1.toFixed(2)}`;
}

/** An intent's dot radius: grows with the square root of the definitions it covers, between 8 and 22. */
export function intentRadius(defs: number): number {
  return Math.min(22, Math.max(8, 8 + 3.2 * Math.sqrt(Math.max(0, defs))));
}

/** The x of the i-th of n dots along a row from `left` to `right`. */
export function rowX(i: number, n: number, left: number, right: number): number {
  return n <= 1 ? (left + right) / 2 : left + (i * (right - left)) / (n - 1);
}

/** The kind of intent as a word. */
export function kindLabel(kind: string): string {
  const names: Record<string, string> = { fix: 'Fix', docs: 'Docs', feat: 'Feature', refactor: 'Refactor', test: 'Tests', chore: 'Chore' };
  return names[kind] ?? (kind ? kind.replace(/^./, (c) => c.toUpperCase()) : 'Change');
}

/** The basis as a word: measured by a check, or estimated by a reviewer. */
export const basisLabel = (basis: string): string => (basis === 'measured' ? 'measured' : 'estimated');
