// The Weft client: the plaza's version control as the owner's console reads it, and the few writes the console
// makes (signing a release, exporting, adopting a remote). Every call goes through the hub's owner door under
// `/v1/weft`; reads answer JSON except the file bytes, which come back as text.

import { call, raw } from './api';

// ---- Types (the shapes weft serve answers; see crates/weft/src/server.rs) ---------------------

/** A change's state: an open lane, sealed for review, in the queue, in conflict, landed on main, or dropped. */
export type WeftChangeState = 'open' | 'sealed' | 'queued' | 'conflict' | 'landed' | 'abandoned';

/** Who signed or did something: the hub, the owner, or an agent by its cid. */
export interface WeftActor {
  kind: string;
  cid?: number;
  name?: string;
}

export interface WeftStatus {
  running: boolean;
  mode: string;
  port: number;
}

/** A git remote a repo exports to: `in-step`, `moved` (the remote changed on its own), `adopting`, or `error`. */
export interface WeftConnection {
  id: string;
  remote: string;
  platform: string;
  branch: string;
  state: string;
  detail: string;
  head: string | null;
  exported: number;
  author: string;
}

/** A release as a repo lists it (name, from, signed or not). */
export interface WeftReleaseRef {
  version: string;
  signed: boolean;
  from: string;
}

export interface WeftRepo {
  name: string;
  project: string | null;
  description: string;
  /** Each ref's head: `main` is the landed revision the head points at (`w-1.l`). */
  refs: Record<string, string>;
  /** Each ref's landing queue, by change name. */
  queues: Record<string, string[]>;
  rules: unknown[];
  connections: WeftConnection[];
  releases: WeftReleaseRef[];
  open_changes: number;
  landed: number;
}

/** One row of a change list (`log`, the queue). */
export interface WeftChangeSummary {
  change: string;
  repo: string;
  ref: string;
  task: string | null;
  title: string;
  author: string;
  state: WeftChangeState;
  kind: string;
  rounds: number;
  drafts: number;
  files: number;
  latest: string | null;
  landed: string | null;
  at_ms: number;
  intents: { title: string; kind: string }[];
  imported: { git: string; author: string } | null;
}

export interface WeftLog {
  repo: string;
  query: string;
  total: number;
  changes: WeftChangeSummary[];
}

export interface WeftIntent {
  title: string;
  kind: string;
  summary?: string;
  covers?: string[];
}

/** A draft or a sealed round of a change, as the change page lists it. */
export interface WeftRevisionRow {
  id: string;
  kind: string;
  n: number;
  base: string;
  files: number;
  digest: string;
  actor: string;
  model: string | null;
  turn: number | null;
  tool_calls: string[];
  seq: number;
  at_ms: number;
}

/** A signed step in a change's sign-off chain. `predicate` carries the step's own fields (summary, node, pass, outcome,
 * decision, notes, reasons, …). */
export interface WeftRecord {
  seq: number;
  type: string;
  revision: string;
  actor: string;
  key: string;
  via_console: boolean;
  at_ms: number;
  predicate: Record<string, unknown>;
}

export interface WeftChange {
  change: string;
  id: number;
  repo: string;
  ref: string;
  task: string | null;
  title: string;
  author: string;
  state: WeftChangeState;
  kind: string;
  base: string;
  stacked_on: string | null;
  reverts: string | null;
  revisions: WeftRevisionRow[];
  drafts: string[];
  rounds: string[];
  landed: string | null;
  /** The intents of each round, keyed by the round number ("1", "2"). */
  intents: Record<string, WeftIntent[]>;
  records: WeftRecord[];
  conflict: unknown;
  queue_position: number | null;
  import: { git: string; author_name: string; author_email: string } | null;
  exported: Record<string, { git: string; intent: string }[]>;
  opened_ms: number;
}

export interface WeftDelta {
  path: string;
  mode: number;
  after: string | null;
  before?: string | null;
}

export interface WeftRevision {
  id: string;
  kind: string;
  change: number;
  base: string;
  n: number;
  seq: number;
  at_ms: number;
  digest: string;
  actor: WeftActor;
  model: string | null;
  turn: number | null;
  tool_calls: string[];
  deltas: WeftDelta[];
}

export interface WeftFileEntry {
  path: string;
  hash: string;
  mode: number;
  size: number;
}

export interface WeftFiles {
  repo: string;
  revision: string;
  files: WeftFileEntry[];
}

/** One definition in a file (a struct, a function, a section), with the agent steps that wrote each part of it. */
export interface WeftProvenance {
  by: WeftActor;
  model: string | null;
  turn: number | null;
  source: string;
  start_line: number;
  end_line: number;
  tool_calls: string[];
}

export interface WeftTouch {
  how: string;
  to: string;
  changed: boolean;
}

export interface WeftSymbol {
  id: string;
  kind: string;
  qualified: string;
  /** `+` added, `~` changed, `-` removed, `>` moved or renamed (from is the old place). */
  op: string;
  from: string | null;
  lineage: string;
  start_line: number;
  end_line: number;
  add: number;
  del: number;
  provenance: WeftProvenance[];
  touches: WeftTouch[];
}

export interface WeftTreeFile {
  path: string;
  op: string;
  before: string | null;
  after: string | null;
  binary: boolean;
  pack: string | null;
  add: number;
  del: number;
  symbols: WeftSymbol[];
}

export interface WeftTreeIntent {
  title: string;
  kind: string;
  summary: string;
  items: string[];
}

/** A revision's change tree: the files it touches, the definitions in them, and the intents they answer to. */
export interface WeftChangeTree {
  change: string;
  revision: string;
  base: string;
  add: number;
  del: number;
  files: WeftTreeFile[];
  intents: WeftTreeIntent[];
  languages: Record<string, number>;
  divergence: unknown[];
  unsorted: unknown[];
}

/** Hunk lines are `[op, text]`, op being `+`, `-` or a space. */
export interface WeftHunk {
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
  lines: [string, string][];
}

export interface WeftDiffFile {
  path: string;
  add: number;
  del: number;
  unified: string;
  hunks: WeftHunk[];
}

export interface WeftDiff {
  base: string;
  revision: string;
  files: WeftDiffFile[];
}

export interface WeftBlameBy {
  change: string;
  source: string;
  start_line: number;
  end_line: number;
  model: string | null;
  turn: number | null;
  tool_calls: string[];
  by: WeftActor;
}

export interface WeftBlameLine {
  line: number;
  text: string;
  by: WeftBlameBy;
}

export interface WeftBlame {
  path: string;
  ref: string;
  lines: WeftBlameLine[];
}

export interface WeftQueue {
  repo: string;
  ref: string;
  head: string;
  queue: WeftChangeSummary[];
}

/** An artifact the repo built (a binary, a bundle), with its provenance statement in `artifact.provenance`. */
export interface WeftArtifact {
  artifact: {
    id: string;
    repo: string;
    name: string;
    version: string | null;
    target: string;
    digest: string;
    size: number;
    built_from: string;
    builder: string;
    command: string;
    provenance: unknown;
    reproduced: boolean;
    change: string | null;
  };
  seq: number;
  at_ms: number;
}

export interface WeftReleaseExport {
  repo: string;
  version: string;
  connection: string;
  tag: string;
  url: string | null;
}

export interface WeftRelease {
  version: string;
  repo: string;
  from: string;
  line: string | null;
  notes: string;
  targets: string[];
  imported: string | null;
  proposed_seq: number;
  /** Set once the owner has signed: when, by which key, over which digest. */
  signed: { seq: number; key: string; digest: string } | null;
  artifacts: WeftArtifact[];
  exports: WeftReleaseExport[];
}

export interface WeftComponent {
  name: string;
  title: string;
  paths: string[];
  languages: string[];
  roles: string[];
}

export interface WeftMap {
  repo: string;
  project: string | null;
  map: { component: WeftComponent[]; description: string; release: unknown[] };
  /** True when the map is a draft from the tree (no map file has been landed yet). */
  drafted: boolean;
  connections: WeftConnection[];
  releases: WeftReleaseRef[];
}

export interface WeftWhereHit {
  component: string | null;
  path: string;
  score: number;
  symbol: string | null;
  why: string;
}

export interface WeftWhere {
  question: string;
  hits: WeftWhereHit[];
}

export interface WeftOp {
  seq: number;
  at_ms: number;
  actor: WeftActor;
  kind: string;
  subject: string | null;
  summary: string;
  undone_by: number | null;
}

export interface WeftVerify {
  ok: boolean;
  owner_key: string;
  subject: string;
  records: { seq: number; kind: string; actor: string; key: string; signer: string; at_ms: number; ok: boolean; via_console: boolean; body: unknown; chain: string[] }[];
}

export interface WeftSigned {
  seq: number;
  summary: string;
}

export interface WeftDigest {
  repo: string;
  version: string;
  artifacts: string[];
  digest: string;
}

// ---- Client ----------------------------------------------------------------

const BASE = '/v1/weft';

/** A query string from the defined parameters, or ''. */
function qs(params: Record<string, string | number | undefined>): string {
  const parts = Object.entries(params)
    .filter(([, v]) => v !== undefined && v !== '')
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(String(v))}`);
  return parts.length ? `?${parts.join('&')}` : '';
}

const seg = (s: string): string => encodeURIComponent(s);

const get = <T>(path: string) => call<T>('GET', `${BASE}${path}`);
const post = <T>(path: string, body: unknown = {}) => call<T>('POST', `${BASE}${path}`, body);

export const weft = {
  /** Whether the weft server runs in this hub, and how. */
  status: () => call<WeftStatus>('GET', BASE),

  /** Every repo the owner can see, with its head, counts, connections and releases. */
  repos: () => get<{ repos: WeftRepo[] }>('/repos').then((r) => r.repos),
  repo: (name: string) => get<WeftRepo>(`/repos/${seg(name)}`),

  /** Changes matching a query (`all()`, `open()`, `needs(owner)`, …), newest first. */
  log: (repo: string, q = 'all()', limit = 200) => get<WeftLog>(`/repos/${seg(repo)}/log${qs({ q, limit })}`),
  change: (name: string) => get<WeftChange>(`/changes/${seg(name)}`),
  revision: (id: string) => get<WeftRevision>(`/revisions/${seg(id)}`),
  files: (rev: string) => get<WeftFiles>(`/revisions/${seg(rev)}/files`),
  /** A file's text at a revision. Binary files fail to decode as text and the call throws. */
  file: async (rev: string, path: string): Promise<string> => {
    const res = await raw('GET', `${BASE}/revisions/${seg(rev)}/file${qs({ path })}`);
    const bytes = await res.arrayBuffer();
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  },
  tree: (rev: string) => get<WeftChangeTree>(`/revisions/${seg(rev)}/tree`),
  diff: (rev: string, path?: string) => get<WeftDiff>(`/revisions/${seg(rev)}/diff${qs({ path })}`),
  blame: (repo: string, path: string, symbol?: string) => get<WeftBlame>(`/repos/${seg(repo)}/blame${qs({ path, symbol })}`),
  queue: (repo: string) => get<WeftQueue>(`/repos/${seg(repo)}/queue`),

  releases: (repo: string) => get<{ releases: WeftRelease[] }>(`/repos/${seg(repo)}/releases`).then((r) => r.releases),
  release: (repo: string, version: string) => get<WeftRelease>(`/repos/${seg(repo)}/releases/${seg(version)}`),
  /** Proposes a release from a landed revision (the head when `from` is omitted). */
  proposeRelease: (repo: string, version: string, from?: string) =>
    post<unknown>(`/repos/${seg(repo)}/releases`, from ? { version, from } : { version }),
  /** The digest the owner signs: over the release's artifacts, by id. */
  releaseDigest: (repo: string, version: string, artifacts: string[]) =>
    post<WeftDigest>(`/repos/${seg(repo)}/releases/${seg(version)}/digest`, { artifacts }),
  /** The owner's signature, through the console (the hub signs for the owner). */
  signRelease: (repo: string, version: string, artifacts: string[], digest: string) =>
    post<WeftSigned>('/console', { kind: 'release.sign', body: { repo, version, artifacts, digest } }),
  exportRelease: (repo: string, version: string, connection: string) =>
    post<WeftReleaseExport>(`/repos/${seg(repo)}/releases/${seg(version)}/export`, { connection }),
  /** Exports the repo's landed changes to a connection; `dryRun` says what would go, without writing. */
  exportRepo: (repo: string, connection: string, dryRun = false) =>
    post<unknown>(`/repos/${seg(repo)}/export`, dryRun ? { connection, dry_run: true } : { connection }),
  /** Brings a moved remote's own commits into the repo, as a change. */
  adopt: (repo: string, connection: string) => post<unknown>(`/repos/${seg(repo)}/adopt`, { connection }),

  map: (repo: string) => get<WeftMap>(`/repos/${seg(repo)}/map`),
  where: (repo: string, q: string) => get<WeftWhere>(`/repos/${seg(repo)}/map/where${qs({ q })}`),

  /** The artifacts a revision built, or those in a release (`repo@version`). */
  artifacts: (query: { revision?: string; release?: string } = {}) =>
    get<{ artifacts: WeftArtifact[] }>(`/artifacts${qs({ revision: query.revision, release: query.release })}`).then((r) => r.artifacts),
  /** The download and the provenance of one artifact, by id (plain links, same origin). */
  artifactContent: (id: string) => `${BASE}/artifacts/${seg(id)}/content`,
  artifactProvenance: (id: string) => `${BASE}/artifacts/${seg(id)}/provenance`,

  /** The signature check of a change (`w-12`) or a release (`repo@version`). */
  verify: (subject: string) => get<WeftVerify>(`/verify${qs({ subject })}`),
  ops: (limit = 50) => get<{ ops: WeftOp[] }>(`/ops${qs({ limit })}`).then((r) => r.ops),
};
