// The History screen's logic without the DOM: the labels, the query presets, a change's sign-off chain, its status
// line, blame runs, the file tree and the routes. Types only come from the API module, so the tests run it under node.

import type { WeftBlameLine, WeftChange, WeftChangeState, WeftChangeSummary, WeftConnection, WeftRecord, WeftRelease, WeftRepo } from '../api-weft';
import type { Tone } from '../ui/components';

// ---- Labels ------------------------------------------------------------------

const CHANGE_STATE: Record<WeftChangeState, { label: string; tone: Tone }> = {
  open: { label: 'Open', tone: 'muted' },
  sealed: { label: 'Sealed', tone: 'info' },
  queued: { label: 'Queued', tone: 'info' },
  conflict: { label: 'Conflict', tone: 'danger' },
  landed: { label: 'Landed', tone: 'ok' },
  abandoned: { label: 'Dropped', tone: 'muted' },
};

export function changeStateLabel(state: string): string {
  return CHANGE_STATE[state as WeftChangeState]?.label ?? state;
}

export function changeStateTone(state: string): Tone {
  return CHANGE_STATE[state as WeftChangeState]?.tone ?? 'muted';
}

/** A connection's state in plain words: in step with its remote, moved on its own, adopting its commits, or an error. */
export function connectionLabel(state: string): string {
  switch (state) {
    case 'in-step':
      return 'In step';
    case 'moved':
      return 'Moved on its own';
    case 'adopting':
      return 'Adopting';
    case 'error':
      return 'Error';
    default:
      return state;
  }
}

export function connectionTone(state: string): Tone {
  switch (state) {
    case 'in-step':
      return 'ok';
    case 'moved':
      return 'warn';
    case 'adopting':
      return 'info';
    case 'error':
      return 'danger';
    default:
      return 'muted';
  }
}

export type ReleaseStatus = 'imported' | 'signed' | 'proposed';

/** A release is imported from a git tag, signed by the owner, or only proposed (waiting for the owner's signature). */
export function releaseStatus(r: Pick<WeftRelease, 'signed' | 'imported'>): ReleaseStatus {
  if (r.imported) return 'imported';
  return r.signed ? 'signed' : 'proposed';
}

export const RELEASE_LABEL: Record<ReleaseStatus, string> = { imported: 'Imported', signed: 'Signed', proposed: 'Proposed' };
export const RELEASE_TONE: Record<ReleaseStatus, Tone> = { imported: 'info', signed: 'ok', proposed: 'warn' };

/** A byte count in words: 512 B, 1.4 KB, 2.3 MB. */
export function sizeText(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? '' : 's'}`;
}

/** An actor label in plain words: `agent:301` is `agent 301`; the hub and the owner keep their names. */
export function displayActor(label: string): string {
  return label.replace(/^agent:/, 'agent ');
}

/** The change a revision belongs to: `w-12.r2` is `w-12`. */
export function changeOfRevision(revision: string): string {
  return revision.split('.')[0] ?? revision;
}

/** The first eight characters of a key id: enough to tell keys apart by eye. */
export function shortKey(key: string): string {
  return key.slice(0, 8);
}

// ---- Query presets -----------------------------------------------------------

export interface QueryPreset {
  key: string;
  label: string;
  query: string;
}

/** The one-click filters above the change list; each is a query of the log's language. */
export const QUERY_PRESETS: QueryPreset[] = [
  { key: 'all', label: 'All', query: 'all()' },
  { key: 'open', label: 'Open', query: 'open()' },
  { key: 'landed', label: 'Landed', query: 'landed()' },
  { key: 'needs', label: 'Needs me', query: 'needs(owner)' },
  { key: 'conflicts', label: 'Conflicts', query: 'conflicts()' },
];

/** A query with its spaces taken out, for comparing with a preset. An empty query is `all()`. */
export function normalizeQuery(query: string): string {
  const q = query.replace(/\s+/g, '');
  return q === '' ? 'all()' : q;
}

/** The preset a query is, or null when it is a query of the reader's own. */
export function presetKey(query: string): string | null {
  const q = normalizeQuery(query);
  return QUERY_PRESETS.find((p) => p.query === q)?.key ?? null;
}

// ---- Sign-off chain ----------------------------------------------------------

export const CHAIN_STEPS = ['written', 'checked', 'review', 'gate', 'ruling', 'landed'] as const;
export type ChainStep = (typeof CHAIN_STEPS)[number];

export const STEP_LABEL: Record<ChainStep, string> = {
  written: 'Written',
  checked: 'Checked',
  review: 'Reviewed',
  gate: 'Gate',
  ruling: 'Ruling',
  landed: 'Landed',
};

/** How a signed step came out: `ok` passed, `warn` sent back or not passed, `danger` failed, `owner` waits for the
 * owner's hand, `pending` has not happened yet. */
export type Outcome = 'ok' | 'warn' | 'danger' | 'owner' | 'pending';

export interface ChainDot {
  step: ChainStep;
  seq: number;
  outcome: Outcome;
  /** The actor label as the record has it (`agent:301`, `hub`, `owner`); the view names it. */
  signer: string;
  at_ms: number;
  /** A ruling signed from the owner's console. */
  console: boolean;
  text: string;
  key: string;
}

export interface ChainStepView {
  step: ChainStep;
  label: string;
  outcome: Outcome;
  dots: ChainDot[];
}

export interface ChainRound {
  round: number;
  steps: ChainStepView[];
}

const STEP_OF_TYPE: Record<string, ChainStep> = {
  written: 'written',
  checked: 'checked',
  review: 'review',
  gate: 'gate',
  ruling: 'ruling',
  landed: 'landed',
};

const RANK: Record<Outcome, number> = { pending: 0, ok: 1, warn: 2, owner: 3, danger: 4 };

const str = (v: unknown): string => (typeof v === 'string' ? v : '');

/** The outcome of one signed record, read from its own fields. */
export function outcomeOf(record: Pick<WeftRecord, 'type' | 'predicate'>): Outcome {
  const p = record.predicate;
  switch (record.type) {
    case 'written':
    case 'checked':
    case 'landed':
      return 'ok';
    case 'review':
      return p.pass === false ? 'warn' : 'ok';
    case 'gate':
      switch (p.outcome) {
        case 'pass':
          return 'ok';
        case 'rework':
          return 'warn';
        case 'owner':
          return 'owner';
        case 'fail':
          return 'danger';
        default:
          return 'pending';
      }
    case 'ruling':
      switch (p.decision) {
        case 'pass':
          return 'ok';
        case 'rework':
          return 'warn';
        case 'fail':
          return 'danger';
        default:
          return 'pending';
      }
    default:
      return 'pending';
  }
}

/** One line for a signed record's hover: what it said, in its own words. */
function textOf(step: ChainStep, r: WeftRecord): string {
  const p = r.predicate;
  switch (step) {
    case 'written':
      return str(p.summary);
    case 'checked':
      return `checked: ${str(p.tool) || str(p.node) || 'hub checks'}`;
    case 'review': {
      const verdict = p.pass === false ? 'not passed' : 'passed';
      const notes = str(p.notes);
      return `${str(p.node) || 'review'} ${verdict}${notes ? `: ${notes}` : ''}`;
    }
    case 'gate': {
      const reasons = Array.isArray(p.reasons) ? p.reasons.filter((x): x is string => typeof x === 'string') : [];
      return `${str(p.outcome) || 'gate'}${reasons.length ? `: ${reasons.join('; ')}` : ''}`;
    }
    case 'ruling': {
      const notes = str(p.notes);
      return `${str(p.decision) || 'ruling'}${notes ? `: ${notes}` : ''}`;
    }
    case 'landed':
      return `on ${str(p.ref) || 'main'}`;
  }
}

function dotOf(step: ChainStep, r: WeftRecord): ChainDot {
  return {
    step,
    seq: r.seq,
    outcome: outcomeOf(r),
    signer: r.actor,
    at_ms: r.at_ms,
    console: r.via_console === true,
    text: textOf(step, r),
    key: r.key,
  };
}

function stepOutcome(dots: ChainDot[]): Outcome {
  if (dots.length === 0) return 'pending';
  return dots.reduce<Outcome>((worst, d) => (RANK[d.outcome] > RANK[worst] ? d.outcome : worst), 'pending');
}

const ROUND_OF = /\.r(\d+)$/;
const LANDED_OF = /\.l$/;

/** A change's sign-off chain, one per round (`w-12.r1`, `w-12.r2`), each in the order written, checked, reviewed, gate,
 * ruling, landed. Records of a round are its own; the landing (`w-12.l`) belongs to the last round. Steps with nothing
 * signed yet are `pending`; a ruling the gate sent to the owner is `owner` until it is signed. */
export function signOffChain(records: WeftRecord[]): ChainRound[] {
  const sorted = [...records].sort((a, b) => a.seq - b.seq);
  let last = 0;
  for (const r of sorted) {
    const m = ROUND_OF.exec(r.revision);
    if (m) last = Math.max(last, Number(m[1]));
  }
  const rounds = new Map<number, WeftRecord[]>();
  for (const r of sorted) {
    const step = STEP_OF_TYPE[r.type];
    if (!step) continue;
    const m = ROUND_OF.exec(r.revision);
    const round = m ? Number(m[1]) : LANDED_OF.test(r.revision) ? last : 0;
    if (round < 1) continue;
    const list = rounds.get(round) ?? [];
    list.push(r);
    rounds.set(round, list);
  }
  return [...rounds.keys()]
    .sort((a, b) => a - b)
    .map((round) => {
      const recs = rounds.get(round) ?? [];
      const steps: ChainStepView[] = CHAIN_STEPS.map((step) => {
        const dots = recs.filter((r) => STEP_OF_TYPE[r.type] === step).map((r) => dotOf(step, r));
        return { step, label: STEP_LABEL[step], outcome: stepOutcome(dots), dots };
      });
      const gate = steps.find((s) => s.step === 'gate');
      const ruling = steps.find((s) => s.step === 'ruling');
      if (ruling && ruling.dots.length === 0 && gate?.dots.at(-1)?.outcome === 'owner') ruling.outcome = 'owner';
      return { round, steps };
    });
}

/** Whether the hub has handed the change to the owner and no ruling has come back since. */
export function waitsForOwner(records: Pick<WeftRecord, 'type' | 'seq' | 'predicate'>[]): boolean {
  let waiting = false;
  for (const r of [...records].sort((a, b) => a.seq - b.seq)) {
    if (r.type === 'gate') waiting = r.predicate.outcome === 'owner';
    else if (r.type === 'ruling') waiting = false;
  }
  return waiting;
}

/** The reasons the gate gave for sending the change to the owner (the last gate's), in words. */
export function ownerReasons(records: Pick<WeftRecord, 'type' | 'seq' | 'predicate'>[]): string {
  const gates = records.filter((r) => r.type === 'gate').sort((a, b) => a.seq - b.seq);
  const reasons = gates.at(-1)?.predicate.reasons;
  return Array.isArray(reasons) ? reasons.filter((x): x is string => typeof x === 'string').join('; ') : '';
}

/** The change's status in one plain line, and whether it needs the owner. */
export function changeStatus(change: Pick<WeftChange, 'state' | 'ref' | 'landed' | 'records'>): { text: string; needsYou: boolean } {
  if (change.state === 'landed') return { text: `Landed on ${change.ref} as ${change.landed ?? 'a revision'}.`, needsYou: false };
  if (waitsForOwner(change.records)) {
    const why = ownerReasons(change.records);
    return { text: `Waiting for your ruling${why ? `: ${why}` : '.'}`, needsYou: true };
  }
  switch (change.state) {
    case 'conflict':
      return { text: `In conflict with ${change.ref}. It needs a rework before it can land.`, needsYou: false };
    case 'queued':
      return { text: `In the queue to land on ${change.ref}.`, needsYou: false };
    case 'sealed':
      return { text: 'Sealed and in review.', needsYou: false };
    case 'abandoned':
      return { text: 'Dropped. It will not land.', needsYou: false };
    default:
      return { text: 'Open: the worker is still writing.', needsYou: false };
  }
}

// ---- Lanes and repos ---------------------------------------------------------

/** The landed changes (the line of main, newest first) and the rest (the open lanes, newest first). */
export function splitLanes(list: WeftChangeSummary[]): { main: WeftChangeSummary[]; lanes: WeftChangeSummary[] } {
  const newest = (a: WeftChangeSummary, b: WeftChangeSummary) => b.at_ms - a.at_ms;
  return {
    main: list.filter((c) => c.state === 'landed').sort(newest),
    lanes: list.filter((c) => c.state !== 'landed').sort(newest),
  };
}

/** The repos a scope shows: the project's, or all of them under All projects. */
export function scopedRepos(repos: WeftRepo[], project: string | null): WeftRepo[] {
  return project === null ? repos : repos.filter((r) => r.project === project);
}

/** The head of main, as the repo names it (`w-1.l`), or `root` before anything landed. */
export function headOf(repo: Pick<WeftRepo, 'refs'>): string {
  return repo.refs.main ?? 'root';
}

/** The connection that needs the owner first: one that moved on its own. */
export function movedConnection(connections: WeftConnection[]): WeftConnection | null {
  return connections.find((c) => c.state === 'moved') ?? null;
}

/** A repo's one line for the owner: a moved remote first, then the rulings waiting, else that nothing waits. */
export function repoLine(
  repo: Pick<WeftRepo, 'name' | 'refs' | 'connections' | 'releases'>,
  waiting: number,
): { text: string; needsYou: boolean } {
  const moved = movedConnection(repo.connections);
  if (moved) return { text: `${moved.id} moved on its own. Adopt it to bring its commits in.`, needsYou: true };
  if (waiting > 0) return { text: `${plural(waiting, 'change')} waiting for your ruling.`, needsYou: true };
  // A proposed release waits for the owner's signature; an imported one never does.
  const unsigned = (repo.releases ?? []).filter((r) => !r.signed && !r.imported).map((r) => r.version);
  if (unsigned.length) return { text: `${unsigned.join(', ')} ${unsigned.length === 1 ? 'waits' : 'wait'} for your signature.`, needsYou: true };
  return { text: `main is at ${headOf(repo)}. Nothing waits for you.`, needsYou: false };
}

/** Releases, newest proposal first. */
export function sortReleases(list: WeftRelease[]): WeftRelease[] {
  return [...list].sort((a, b) => b.proposed_seq - a.proposed_seq);
}

// ---- Blame -------------------------------------------------------------------

export interface BlameRun {
  from: number;
  to: number;
  change: string;
  source: string;
  actorKind: string;
  cid: number | null;
  model: string | null;
  turn: number | null;
}

/** Consecutive lines written by the same change, draft and actor, as one run (the gutter shows each run once). */
export function blameRuns(lines: WeftBlameLine[]): BlameRun[] {
  const runs: BlameRun[] = [];
  for (const line of lines) {
    const by = line.by;
    const cid = by.by.cid ?? null;
    const last = runs.at(-1);
    if (last && last.change === by.change && last.source === by.source && last.actorKind === by.by.kind && last.cid === cid) {
      last.to = line.line;
    } else {
      runs.push({ from: line.line, to: line.line, change: by.change, source: by.source, actorKind: by.by.kind, cid, model: by.model, turn: by.turn });
    }
  }
  return runs;
}

// ---- Files -------------------------------------------------------------------

export interface FileNode {
  name: string;
  path: string;
  dir: boolean;
  size: number;
  children: FileNode[];
}

/** The files as a folder tree: folders first, each level by name. */
export function fileTree(files: { path: string; size: number }[]): FileNode[] {
  const root: FileNode = { name: '', path: '', dir: true, size: 0, children: [] };
  for (const f of files) {
    const parts = f.path.split('/').filter(Boolean);
    let node = root;
    let at = '';
    parts.forEach((name, i) => {
      at = at ? `${at}/${name}` : name;
      const last = i === parts.length - 1;
      let child = node.children.find((c) => c.name === name && c.dir === !last);
      if (!child) {
        child = { name, path: at, dir: !last, size: last ? f.size : 0, children: [] };
        node.children.push(child);
      }
      node = child;
    });
  }
  const order = (nodes: FileNode[]): FileNode[] =>
    nodes
      .map((n) => ({ ...n, children: order(n.children) }))
      .sort((a, b) => (a.dir === b.dir ? (a.name < b.name ? -1 : a.name > b.name ? 1 : 0) : a.dir ? -1 : 1));
  return order(root.children);
}

// ---- Routes ------------------------------------------------------------------

export type HistoryTab = 'changes' | 'releases' | 'map' | 'files';
export const HISTORY_TABS: { key: HistoryTab; label: string }[] = [
  { key: 'changes', label: 'Changes' },
  { key: 'releases', label: 'Releases' },
  { key: 'map', label: 'Map' },
  { key: 'files', label: 'Files' },
];

export type HistoryRoute = { view: 'repos' } | { view: 'repo'; repo: string; tab: HistoryTab } | { view: 'change'; repo: string; change: string };

/** `#/history` is the repos; `#/history/<repo>[/<tab>]` a repo; `#/history/<repo>/change/<w-N>` a change. */
export function parseHistoryRoute(params: string[]): HistoryRoute {
  const repo = params[0] ?? '';
  if (!repo) return { view: 'repos' };
  if (params[1] === 'change') return { view: 'change', repo, change: params[2] ?? '' };
  const tab = HISTORY_TABS.find((t) => t.key === params[1])?.key ?? 'changes';
  return { view: 'repo', repo, tab };
}

/** Diff hunk line colour class from its operator. */
export function diffClass(op: string): 'add' | 'del' | 'ctx' {
  return op === '+' ? 'add' : op === '-' ? 'del' : 'ctx';
}

/** The change's latest round, or its newest draft: the revision whose tree and diff the expert view shows. */
export function latestRevision(change: Pick<WeftChange, 'rounds' | 'drafts' | 'landed'>): string | null {
  return change.rounds.at(-1) ?? change.drafts.at(-1) ?? change.landed ?? null;
}

/** The summary row of a change list, as a flat list of intent titles (for chips). */
export function intentChips(change: Pick<WeftChangeSummary, 'intents'>, max = 3): { title: string; kind: string }[] {
  return change.intents.slice(0, max);
}
