// History: the plaza's version control (Weft) as the owner reads it, drawn as the Loom prototype draws it. The
// repositories, each with its changes as the line of main and the open lanes beside it; a change, with its sign-off
// chain and (in Expert) its revisions, records, tree and diff; the releases (the owner signs and exports them here);
// the map; and the files at main with blame. Read by polling every few seconds: the live bus does not carry the weft
// events, and each read is cheap.

import '../styles/proto/history.css';
import {
  weft,
  type WeftActor,
  type WeftArtifact,
  type WeftBlameLine,
  type WeftChangeTree,
  type WeftComponent,
  type WeftConnection,
  type WeftDiff,
  type WeftExport,
  type WeftIntent,

  type WeftVerify,
  type WeftRecord,
  type WeftRelease,
  type WeftRepo,
  type WeftRevisionRow,
  type WeftSymbol,
  type WeftWhereHit,
} from '../api-weft';
import { agentLabel, agentName } from '../roles';
import { href } from '../router';
import { projectTitle, scope } from '../scope';
import { agents } from '../store';
import { ago, attempt, badge, card, clip, empty, field, modal, select, toast, type Tone } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import { landingGraph, laneGraph, signOffView } from './history-graph';
import {
  HISTORY_TABS,
  QUERY_HELP,
  QUERY_PRESETS,
  RELEASE_LABEL,
  RELEASE_TONE,
  blameRuns,
  changeOfRevision,
  changeStateLabel,
  changeStateTone,
  changeStatus,
  connectionLabel,
  connectionTone,
  diffClass,
  fileTree,
  headOf,
  latestRevision,
  movedConnection,
  normalizeQuery,
  parseHistoryRoute,
  plural,
  queryError,
  releaseStatus,
  repoLine,
  scopedRepos,
  shortKey,
  signOffChain,
  sizeText,
  sortReleases,
  splitLanes,
  type FileNode,

  type HistoryTab,
} from './history-model';
import type { Cleanup } from './index';

/** How often the screen rereads what it shows. */
const POLL_MS = 5_000;
/** The most lines a file shows; a longer one says how many it has. */
const MAX_LINES = 4000;
const EXPERT_KEY = 'apz.history.expert';

/** The sign of each change in a diff or a tree: `+` added, `~` changed, `-` removed, `>` moved or renamed. */
const OP_TONE: Record<string, Tone> = { '+': 'ok', '~': 'info', '-': 'danger', '>': 'accent' };
const OP_WORD: Record<string, string> = { '+': 'added', '~': 'changed', '-': 'removed', '>': 'moved or renamed' };

/** One drawn screen: its markup and the signature of the data it was drawn from (a poll redraws only on a change). */
interface Built {
  sig: string;
  node: HTMLElement;
}

interface Ctx {
  root: HTMLElement;
  route: WeftRoute;
  alive: boolean;
  /** An action is in flight: polls wait for it. */
  busy: boolean;
  gen: number;
  seen: string;
  /** The changes query as committed, and as typed so far (a poll must not lose the typing). */
  query: string;
  draft: string;
  /** Saved queries (chips for this visit) and whether the query help is open. */
  saved: string[];
  help: boolean;
  expert: boolean;
  /** The file open in Files, and whether its blame gutter shows. */
  file: string;
  blame: boolean;
  /** The last "Where is…" question and its hits; the draft as typed. */
  where: { q: string; hits: WeftWhereHit[] } | null;
  whereDraft: string;
  /** Immutable reads (a revision's tree and diff, a file at a revision, blame at a head), kept between polls. */
  cache: Map<string, unknown>;
}

/** The repository tabs the prototype lists, in its order: the four of the history model, then Queue, Trace, Keys, Sync, Ops. */
type WeftTab = HistoryTab | 'queue' | 'trace' | 'keys' | 'sync' | 'ops';
type WeftRoute = { view: 'repos' } | { view: 'repo'; repo: string; tab: WeftTab } | { view: 'change'; repo: string; change: string };
const WEFT_TABS: { key: WeftTab; label: string }[] = [
  { key: 'changes', label: 'Changes' },
  { key: 'queue', label: 'Queue' },
  { key: 'releases', label: 'Releases' },
  { key: 'map', label: 'Map' },
  { key: 'files', label: 'Files' },
  { key: 'trace', label: 'Trace' },
  { key: 'keys', label: 'Keys' },
  { key: 'sync', label: 'Sync' },
  { key: 'ops', label: 'Ops' },
];

/** The route of `#/history/...`, with the tabs the history model does not list (queue, trace, keys, sync, ops). */
function routeOf(params: string[]): WeftRoute {
  const extra = WEFT_TABS.find((t) => t.key === params[1] && !HISTORY_TABS.some((x) => x.key === t.key));
  if (params[0] && extra) return { view: 'repo', repo: params[0], tab: extra.key };
  return parseHistoryRoute(params);
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const c: Ctx = {
    root,
    route: routeOf(params),
    alive: true,
    busy: false,
    gen: 0,
    seen: '',
    query: 'all()',
    draft: 'all()',
    saved: [],
    help: false,
    expert: readExpert(),
    file: '',
    blame: false,
    where: null,
    whereDraft: '',
    cache: new Map(),
  };
  replace(root, wrap(h('p.hx-faint', null, 'Loading…')));
  void draw(c, true);
  const timer = window.setInterval(() => void draw(c, false), POLL_MS);
  return () => {
    c.alive = false;
    window.clearInterval(timer);
  };
}

async function draw(c: Ctx, force: boolean): Promise<void> {
  if (!c.alive || c.busy) return;
  const gen = ++c.gen;
  let built: Built;
  try {
    built = await build(c);
  } catch (err) {
    built = { sig: `error:${errMsg(err)}`, node: wrap(empty('git', 'Weft could not be read', errMsg(err))) };
  }
  // A newer draw has started, or the screen has gone: this one is stale.
  if (!c.alive || gen !== c.gen) return;
  if (!force && built.sig === c.seen) return;
  c.seen = built.sig;
  replace(c.root, built.node);
}

function build(c: Ctx): Promise<Built> {
  const r = c.route;
  if (r.view === 'repos') return reposView();
  if (r.view === 'change') return changeView(c, r.repo, r.change);
  return repoView(c, r.repo, r.tab);
}

/** Runs an action with the screen held still, then redraws it from the server's new state. */
async function act(c: Ctx, run: () => Promise<unknown>, success: string): Promise<void> {
  c.busy = true;
  try {
    await attempt(run, success);
  } finally {
    c.busy = false;
    c.seen = '';
    void draw(c, true);
  }
}

async function cached<T>(c: Ctx, key: string, load: () => Promise<T>): Promise<T> {
  if (c.cache.has(key)) return c.cache.get(key) as T;
  const value = await load();
  c.cache.set(key, value);
  return value;
}

// ---- Names and small parts ---------------------------------------------------

function nameOfCid(cid: number | null, kind: string): string {
  if (cid === null) return kind;
  const agent = agents.get().find((a) => a.cid === cid);
  return agent ? agentName(agent) : agentLabel(cid);
}

/** An actor label as a name: `agent:301` is the agent's name; the hub and the owner keep their words. */
function nameOfLabel(label: string): string {
  const m = /^agent:(\d+)$/.exec(label);
  return m ? nameOfCid(Number(m[1]), 'agent') : label;
}

function nameOfActor(by: WeftActor): string {
  return nameOfCid(by.cid ?? null, by.kind);
}

function errMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function readExpert(): boolean {
  try {
    return localStorage.getItem(EXPERT_KEY) === '1';
  } catch {
    return false;
  }
}

function writeExpert(on: boolean): void {
  try {
    localStorage.setItem(EXPERT_KEY, on ? '1' : '0');
  } catch {
    // A private window keeps no storage: the switch still works for this visit.
  }
}

function wrap(...children: (Node | null)[]): HTMLElement {
  return h('div.history', null, ...children);
}

/** A status line of the page: the words, with an icon for a line that needs the owner or warns of a moved remote. */
function statusLine(cls: string, icon: Node | null, text: Node | string): HTMLElement {
  return h('p.hx-status', { class: cls || null }, icon, icon ? h('span', null, text) : text);
}

/** A button in the prototype's markup: `button.btn` with its icon and label. */
function btn(label: string, onClick: () => void, o: { kind?: string; sm?: boolean; icon?: Parameters<typeof lu>[0]; title?: string; pressed?: boolean; disabled?: boolean } = {}): HTMLButtonElement {
  const b = h(
    'button.btn',
    { type: 'button', class: [o.kind, o.sm ? 'sm' : ''].filter(Boolean).join(' ') || null, title: o.title ?? null, disabled: !!o.disabled },
    o.icon ? lu(o.icon, o.sm ? 14 : 15) : null,
    label ? h('span', null, label) : null,
  );
  if (o.pressed !== undefined) b.setAttribute('aria-pressed', String(o.pressed));
  b.addEventListener('click', onClick);
  return b;
}

/** A link styled as a button: an artifact's download and provenance. */
function linkBtn(label: string, url: string, icon: Parameters<typeof lu>[0]): HTMLElement {
  return h('a.btn.ghost.sm', { href: url, target: '_blank', rel: 'noopener' }, lu(icon, 14), h('span', null, label));
}

/** A card with the prototype's classes, its title as the h3 and its body; `cls` adds a class to its section. */
function panel(title: string, body: Node | Node[], o: { actions?: Node[]; cls?: string } = {}): HTMLElement {
  const el = card(title, body, { actions: o.actions });
  if (o.cls) el.classList.add(o.cls);
  return el;
}

/** The defined children of a card or a row, in order (a null stands for nothing). */
function list(...items: (Node | null | undefined | false)[]): Node[] {
  return items.filter((x): x is Node => !!x);
}

function connBadge(c: WeftConnection): HTMLElement {
  const el = badge(`${c.id} · ${connectionLabel(c.state)}`, connectionTone(c.state));
  if (c.detail) el.title = c.detail;
  return el;
}

// ---- Repositories ------------------------------------------------------------

async function reposView(): Promise<Built> {
  const status = await weft.status().catch(() => null);
  if (status && !status.running) {
    return { sig: 'off', node: wrap(empty('git', 'Weft is not running', 'Start it on the hub to see the plaza’s repositories.')) };
  }
  const repos = scopedRepos(await weft.repos(), scope.get());
  const waiting = await Promise.all(repos.map((r) => weft.log(r.name, 'needs(owner)', 200).then((l) => l.total).catch(() => 0)));
  const total = waiting.reduce((a, b) => a + b, 0);
  // Proposed releases wait for the owner's signature (imported ones never do).
  const unsigned = repos.reduce((n, r) => n + (r.releases ?? []).filter((x) => !x.signed && !x.imported).length, 0);
  const needs = total + unsigned > 0;
  const line = total
    ? `${plural(total, 'change')} waiting for your ruling.`
    : unsigned
      ? `${plural(unsigned, 'release')} waiting for your signature.`
      : repos.length
        ? `${repos.length} ${repos.length === 1 ? 'repository' : 'repositories'}. Nothing waits for you.`
        : 'No repositories yet.';
  const head = h('p.hx-line', { class: needs ? 'is-needs' : null }, needs ? lu('shield-check', 16) : null, h('span', null, line));
  const body = repos.length
    ? h(
        'div.hx-repos',
        null,
        repos.map((r, i) => repoCard(r, waiting[i] ?? 0)),
      )
    : empty('git', 'No repositories yet', 'A project’s repository shows here once it has one.');
  return { sig: JSON.stringify([repos, waiting]), node: wrap(head, body) };
}

function repoCard(r: WeftRepo, waiting: number): HTMLElement {
  const fig = (v: number, label: string): HTMLElement =>
    h('div.hx-fig', null, h('span.hx-fig-v', null, String(v)), h('span.hx-fig-l', null, label));
  return h(
    'a.card.hx-repo',
    { href: href('history', r.name) },
    h(
      'div.card-h.hx-repo-h',
      null,
      lu('git-fork', 16),
      h('h2', null, r.name),
      r.project ? h('span.hx-repo-proj', null, projectTitle(r.project)) : null,
    ),
    h(
      'div.card-b',
      null,
      r.description ? h('p.hx-desc', null, r.description) : null,
      h('div.hx-figs', null, fig(r.landed, 'landed changes'), fig(r.open_changes, 'open'), fig(waiting, 'waiting for you')),
      h('span.hx-mono-chip', null, `main at ${headOf(r)}`),
      ...r.connections.filter((x) => x.state !== 'in-step' && x.detail).map((x) => h('p.hx-faint', null, `${x.id}: ${x.detail}`)),
      r.connections.length ? h('div.hx-conns', null, r.connections.map(connBadge)) : null,
    ),
    waiting > 0 ? h('div.hx-foot', null, `${waiting} waiting for you`) : null,
  );
}

// ---- One repository ----------------------------------------------------------

async function repoView(c: Ctx, name: string, tab: WeftTab): Promise<Built> {
  const [repo, waiting] = await Promise.all([weft.repo(name), weft.log(name, 'needs(owner)', 200)]);
  const moved = movedConnection(repo.connections);
  const line = repoLine(repo, waiting.total);
  const status = moved
    ? statusLine('is-moved', lu('triangle-alert', 16), line.text)
    : line.needsYou
      ? statusLine('is-needs', null, line.text)
      : h('p.hx-status', null, 'main is at ', h('code', null, headOf(repo)), '. Nothing waits for you.');
  const body = await tabBody(c, repo, tab);
  const tabLinks = WEFT_TABS.map((t) =>
    h('a', { href: href('history', name, t.key), 'aria-current': t.key === tab ? 'page' : null }, t.label),
  );
  const node = wrap(
    h(
      'div.view-head',
      null,
      h('div.hx-titlebox', null, h('a.hx-back', { href: href('history') }, '← Repositories'), h('h1', null, repo.name)),
    ),
    repo.description ? h('p.hx-desc', null, repo.description) : null,
    status,
    h('nav.hx-tabs', { 'aria-label': 'Repository' }, tabLinks),
    body.node,
  );
  return { sig: JSON.stringify([repo, waiting.total, tab, body.sig]), node };
}

// Changes: the query box with its help, the presets and saved queries, then the lane graph.

async function changesBody(c: Ctx, repo: WeftRepo): Promise<Built> {
  // The landings as commits, and the open lanes beside them (21.6); the list below filters by query.
  const history = await weft.history(repo.name).catch(() => null);
  const q = c.query.trim() || 'all()';
  const syntax = queryError(q);
  const rows = syntax ? null : await weft.log(repo.name, q, 200).then((l) => l).catch((err) => ({ error: errMsg(err) }));
  const error = syntax ? `Character ${syntax.pos + 1}: ${syntax.message}` : rows && 'error' in rows ? rows.error : null;
  const log = rows && !('error' in rows) ? rows : null;
  const { main, lanes } = log ? splitLanes(log.changes) : { main: [], lanes: [] };
  const active = normalizeQuery(q);

  const field = h('input.input', {
    id: `hx-q-${repo.name}`,
    type: 'text',
    value: c.draft,
    placeholder: 'Filter, e.g. lane(w-2) | needs(owner)',
    'aria-label': 'Query',
  });
  field.addEventListener('input', () => {
    c.draft = field.value;
  });
  field.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter') return;
    commit(c, field.value);
  });

  const helpBtn = h(
    'button.iconbtn',
    { type: 'button', 'aria-expanded': String(c.help), 'aria-label': 'Query help', title: 'Query help' },
    lu('circle-help', 16),
  );
  helpBtn.addEventListener('click', () => {
    c.help = !c.help;
    c.seen = '';
    void draw(c, true);
  });
  const help = h(
    'div.hx-qhelp',
    null,
    helpBtn,
    c.help ? helpPanel(c, repo.name) : null,
  );

  const presetChip = (label: string, query: string): HTMLElement => {
    const on = normalizeQuery(query) === active;
    const b = h('button.hx-qchip', { type: 'button', 'aria-pressed': String(on) }, label);
    b.addEventListener('click', () => {
      c.query = query;
      c.draft = query;
      c.help = false;
      c.seen = '';
      void draw(c, true);
    });
    return b;
  };
  const chips = [
    ...QUERY_PRESETS.map((p) => presetChip(p.label, p.query)),
    ...c.saved.map((s) => presetChip(s, s)),
  ];
  const save = btn('Save this query', () => saveQuery(c), { kind: 'ghost', sm: true });

  const bar = h(
    'div.hx-qbar',
    null,
    h('div.hx-bar', null, field, help),
    h('div.hx-qrow', { role: 'group', 'aria-label': 'Saved queries' }, chips, save),
    error ? h('p.hx-qerr', { role: 'alert' }, error) : null,
  );
  if (error) return { sig: JSON.stringify(['error', error, c.query]), node: h('div.hx-changes', null, bar, h('p.hx-faint', null, 'Fix the query to see changes.')) };
  if (!log || log.changes.length === 0) {
    return { sig: JSON.stringify(['empty', c.query]), node: h('div.hx-changes', null, bar, h('p.hx-faint', null, 'No changes match this query.')) };
  }
  const graph = laneGraph(repo.name, main, lanes, headOf(repo), nameOfLabel);
  return { sig: JSON.stringify([log.changes, c.query, history?.head ?? null, history?.lanes.length ?? 0]), node: h('div.hx-changes', null, bar, history ? landingGraph(history) : null, graph) };
}

/** A query commits from the box: Enter runs the typed text. */
function commit(c: Ctx, text: string): void {
  c.query = text.trim();
  c.draft = text;
  c.seen = '';
  void draw(c, true);
}

/** Save this query: the box's text becomes a chip for this visit; a query that does not parse is refused with the reason. */
function saveQuery(c: Ctx): void {
  const q = c.draft.trim();
  if (!q) {
    toast('Type a query first.', 'error');
    return;
  }
  const bad = queryError(q);
  if (bad) {
    toast(`Fix the query first: ${bad.message}`, 'error');
    return;
  }
  if (QUERY_PRESETS.some((p) => normalizeQuery(p.query) === normalizeQuery(q)) || c.saved.some((s) => normalizeQuery(s) === normalizeQuery(q))) {
    toast('That query is already a chip.');
    return;
  }
  c.saved.push(q);
  toast(`Saved ${q}`);
  c.seen = '';
  void draw(c, true);
}

/** The query help: the operators and functions, each with an example that the box runs. */
function helpPanel(c: Ctx, repo: string): HTMLElement {
  return h(
    'div.hx-qpop',
    { role: 'dialog', 'aria-label': 'Query help' },
    QUERY_HELP.map((section) =>
      h(
        'div',
        null,
        h('p.hx-qh', null, section.title),
        h(
          'ul.hx-qlist',
          null,
          section.items.map(([sig, desc, ex]) =>
            h(
              'li',
              null,
              h('span', null, h('code', null, sig), ` ${desc}`),
              exampleButton(c, repo, ex),
            ),
          ),
        ),
      ),
    ),
  );
}

function exampleButton(c: Ctx, repo: string, example: string): HTMLElement {
  const b = h('button.hx-qex', { type: 'button', 'data-id': repo }, example);
  b.addEventListener('click', () => {
    c.query = example;
    c.draft = example;
    c.help = false;
    c.seen = '';
    void draw(c, true);
  });
  return b;
}

// Releases: the versions, their notes and artifacts; a proposed one is signed by the owner here, a signed one exported.

async function releasesBody(c: Ctx, repo: WeftRepo): Promise<Built> {
  const releases = sortReleases(await weft.releases(repo.name));
  if (!releases.length) {
    return { sig: 'none', node: empty('git', 'No releases yet', 'A release is proposed from a landed revision, then signed here.') };
  }
  const candidates = new Map<string, WeftArtifact[]>();
  await Promise.all(
    releases
      .filter((r) => releaseStatus(r) === 'proposed')
      .map(async (r) => {
        candidates.set(r.version, await weft.artifacts({ revision: r.from }).catch(() => []));
      }),
  );
  const node = h(
    'div.hst2-stack',
    null,
    releases.map((r) => releaseCard(c, repo, r, candidates.get(r.version) ?? [])),
  );
  return { sig: JSON.stringify([releases, [...candidates.entries()]]), node };
}

function releaseCard(c: Ctx, repo: WeftRepo, r: WeftRelease, candidates: WeftArtifact[]): HTMLElement {
  const status = releaseStatus(r);
  const artifacts = status === 'signed' || status === 'imported' ? r.artifacts : candidates;
  const ids = artifacts.map((a) => a.artifact.id);
  const signBtn =
    status === 'proposed'
      ? btn(`Sign ${r.version}`, () => void signRelease(c, repo.name, r, ids), {
          kind: 'primary',
          title: 'Sign this release with your key: its artifacts and their digest',
        })
      : null;
  const exportArea = repo.connections.length
    ? exportControl(c, repo.name, r, repo.connections, status === 'signed')
    : h('span.hint', null, 'No connection to export to yet.');
  const artList = artifacts.length
    ? h(
        'ul.hst2-arts',
        null,
        artifacts.map((a) => artifactRow(a)),
      )
    : h('p.hint', null, status === 'proposed' ? 'No artifacts built from this revision yet.' : 'No artifacts.');
  const exports = r.exports.length
    ? h(
        'ul.hst2-exports',
        null,
        r.exports.map((e) =>
          h(
            'li',
            null,
            'Exported to ',
            h('code', null, e.connection),
            ' as ',
            h('code', null, e.tag),
            e.url && /^https?:\/\//.test(e.url) ? h('a', { href: e.url, target: '_blank', rel: 'noopener' }, ' Open') : null,
          ),
        ),
      )
    : null;
  return panel(
    r.version,
    list(
      h(
        'p.hst2-from',
        null,
        'from ',
        h('a', { href: href('history', repo.name, 'change', changeOfRevision(r.from)) }, h('code', null, r.from)),
        r.line ? ' · line ' : null,
        r.line ? h('code', null, r.line) : null,
      ),
      r.notes.trim() ? h('div.hst2-notes', null, renderMarkdown(r.notes)) : null,
      h('h4.hst2-sub', null, 'Artifacts'),
      artList,
      r.signed ? h('p.hst2-signed', null, 'Signed by key ', h('code', null, shortKey(r.signed.key)), ' · digest ', h('code', null, shortKey(r.signed.digest))) : null,
      h('div.hst2-actions', null, signBtn, exportArea),
      exports,
    ),
    { actions: [badge(RELEASE_LABEL[status], RELEASE_TONE[status], true)], cls: 'hst2-rel' },
  );
}

function artifactRow(a: WeftArtifact): HTMLElement {
  const x = a.artifact;
  const meta = [x.target, sizeText(x.size)].filter(Boolean).join(' · ');
  return h(
    'li',
    { class: 'hst2-art' },
    h(
      'div.hst2-art-main',
      null,
      h('strong', null, x.name),
      x.version ? h('span', null, ' ', h('code', null, x.version)) : null,
      h('div.hst2-art-meta.muted', null, meta),
    ),
    h(
      'div.hst2-art-side',
      null,
      x.reproduced ? badge('Reproduced', 'ok') : null,
      linkBtn('Download', weft.artifactContent(x.id), 'download'),
      linkBtn('Provenance', weft.artifactProvenance(x.id), 'scroll-text'),
    ),
  );
}

/** The export control for a release: enabled once the owner has signed it; with several connections, a choice first. */
function exportControl(c: Ctx, repo: string, r: WeftRelease, connections: WeftConnection[], signed: boolean): HTMLElement {
  const first = connections[0]?.id ?? '';
  const choice = connections.length > 1 ? select(connections.map((x) => ({ value: x.id, label: x.id })), first) : null;
  const label = (id: string) => `Export to ${id}`;
  const exportBtn = btn(label(first), () => void exportTo(c, repo, r, choice?.value ?? first), {
    kind: 'primary',
    disabled: !signed,
    title: signed ? undefined : 'Sign the release first',
  });
  if (choice) {
    choice.addEventListener('change', () => {
      exportBtn.querySelector('span')!.textContent = label(choice.value);
    });
  }
  return h('span.hst2-export', null, choice, exportBtn);
}

async function signRelease(c: Ctx, repo: string, r: WeftRelease, ids: string[]): Promise<void> {
  await act(
    c,
    async () => {
      const digest = await weft.releaseDigest(repo, r.version, ids);
      await weft.signRelease(repo, r.version, ids, digest.digest);
    },
    `${r.version} is signed`,
  );
}

async function exportTo(c: Ctx, repo: string, r: WeftRelease, connection: string): Promise<void> {
  if (!connection) return;
  await act(c, () => weft.exportRelease(repo, r.version, connection), `${r.version} exported to ${connection}`);
}

// Map: the components, the connections, and "Where is…".

async function mapBody(c: Ctx, repo: WeftRepo): Promise<Built> {
  const m = await weft.map(repo.name);
  const comps = m.map.component;
  const compsEl = comps.length
    ? h(
        'ul.hst2-comps',
        null,
        comps.map((x) => componentEl(x)),
      )
    : empty('file', 'No components yet', 'Components come from the files the repository holds.');
  const moved = movedConnection(m.connections);
  const connsEl = m.connections.length
    ? h(
        'ul.hst2-conns',
        null,
        m.connections.map((x) => connectionRow(c, repo.name, x)),
      )
    : h('p.hint', null, 'No connection. Export goes nowhere until one is added.');
  const node = h(
    'div.hst2-stack',
    null,
    panel('Components', list(m.drafted ? h('p.hint', null, 'Drafted from the files on main. No map has landed yet.') : null, compsEl)),
    panel('Connections', list(moved ? h('p.hint', null, `${moved.id} moved on its own. Adopt its commits to bring them in.`) : null, connsEl)),
    panel('Where is…', list(whereBox(c, repo.name), whereHits(c))),
  );
  return { sig: JSON.stringify([m, c.where, c.whereDraft]), node };
}

function componentEl(x: WeftComponent): HTMLElement {
  const shown = x.paths.slice(0, 4);
  return h(
    'li',
    { class: 'hst2-comp' },
    h('h3', null, x.title || x.name),
    x.title ? h('code', null, x.name) : null,
    h(
      'div.hst2-paths',
      null,
      shown.map((p) => h('code', null, p)),
      x.paths.length > shown.length ? h('span.muted', null, ` +${x.paths.length - shown.length}`) : null,
    ),
    h('div.hst2-langs', null, x.languages.map((l) => badge(l, 'muted'))),
  );
}

function connectionRow(c: Ctx, repo: string, x: WeftConnection): HTMLElement {
  const adopt = x.state === 'moved' ? btn('Adopt', () => void act(c, () => weft.adopt(repo, x.id), `Adopting the commits of ${x.id}`), { kind: 'primary', sm: true }) : null;
  return h(
    'li',
    { class: 'hst2-conn' },
    h('div.hst2-conn-main', null, h('strong', null, x.id), badge(connectionLabel(x.state), connectionTone(x.state), true), h('span.muted', null, `${x.platform} · ${x.branch}`)),
    h('code.hst2-remote', { title: x.remote }, clip(x.remote, 90)),
    x.detail ? h('span.muted', null, x.detail) : null,
    adopt,
  );
}

function whereBox(c: Ctx, repo: string): HTMLElement {
  const q = h('input.input', { type: 'search', placeholder: 'Where is the queue?', value: c.whereDraft, 'aria-label': 'Where is' });
  q.addEventListener('input', () => {
    c.whereDraft = q.value;
  });
  const find = async () => {
    const question = c.whereDraft.trim();
    if (!question) {
      toast('Type what you are looking for first', 'error');
      return;
    }
    try {
      const res = await weft.where(repo, question);
      c.where = { q: question, hits: res.hits };
    } catch (err) {
      toast(errMsg(err), 'error');
    }
    c.seen = '';
    void draw(c, true);
  };
  q.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') void find();
  });
  return h('div.hst2-where', null, q, btn('Find', () => void find(), { kind: 'primary' }));
}

function whereHits(c: Ctx): HTMLElement | null {
  if (!c.where) return null;
  if (!c.where.hits.length) return h('p.hint', null, `Nothing for “${c.where.q}”.`);
  return h(
    'div',
    null,
    h('p.hst2-hitq.muted', null, c.where.q),
    h(
      'ul.hst2-hits',
      null,
      c.where.hits.map((hit) =>
        h(
          'li',
          { class: 'hst2-hit' },
          h('code', null, hit.path),
          hit.symbol ? h('span', null, ` ${hit.symbol}`) : null,
          h('span.muted', null, ` · ${hit.why}`),
          hit.component ? ` ${badge(hit.component, 'info')}` : null,
        ),
      ),
    ),
  );
}

// Files: the folder tree at main, a file read in place, with blame in its gutter when asked.

async function filesBody(c: Ctx, repo: WeftRepo): Promise<Built> {
  const head = headOf(repo);
  if (head === 'root') {
    return { sig: 'root', node: empty('file', 'Nothing landed yet', 'Files appear here once a change lands on main.') };
  }
  const listing = await weft.files(head);
  const tree = fileTree(listing.files);
  const file = c.file ? listing.files.find((f) => f.path === c.file) : undefined;
  let viewer: HTMLElement = h('div.hst2-noview', null, lu('file-text', 28), h('p', null, 'Choose a file to read it.'));
  let text: string | null = null;
  let lines: WeftBlameLine[] | null = null;
  if (c.file && !file) viewer = h('p.hint', null, 'No such file on main.');
  if (file) {
    text = await readText(c, head, file.path);
    if (c.blame && text !== null) lines = await blameOf(c, repo.name, file.path, head);
    viewer = fileViewer(c, file.path, text, lines);
  }
  const sig = JSON.stringify([listing.files.map((f) => `${f.path}${f.hash}`), c.file, c.blame, text, lines]);
  return {
    sig,
    node: h('div.hst2-files', null, h('aside.hst2-tree', { 'aria-label': 'Files' }, treeNodes(c, tree, 0)), h('section.hst2-view', null, viewer)),
  };
}

function treeNodes(c: Ctx, nodes: FileNode[], depth: number): HTMLElement {
  const dirs = nodes.filter((n) => n.dir);
  const files = nodes.filter((n) => !n.dir);
  return h(
    'div',
    null,
    dirs.map((n) =>
      h('details.hst2-dir', depth < 2 ? { open: true } : {}, h('summary', null, n.name), h('div.hst2-kids', null, treeNodes(c, n.children, depth + 1))),
    ),
    files.map((n) => {
      const on = c.file === n.path;
      const b = h(
        'button.hst2-file',
        { type: 'button', class: on ? 'on' : null, 'aria-current': on ? 'true' : null, onclick: () => openFile(c, n.path) },
        h('span', null, n.name),
        h('span.muted', null, sizeText(n.size)),
      );
      return b;
    }),
  );
}

function openFile(c: Ctx, path: string): void {
  c.file = path;
  c.seen = '';
  void draw(c, true);
}

async function readText(c: Ctx, rev: string, path: string): Promise<string | null> {
  try {
    return await cached(c, `text|${rev}|${path}`, () => weft.file(rev, path));
  } catch {
    return null;
  }
}

async function blameOf(c: Ctx, repo: string, path: string, head: string): Promise<WeftBlameLine[] | null> {
  try {
    return (await cached(c, `blame|${repo}|${path}|${head}`, () => weft.blame(repo, path))).lines;
  } catch {
    return null;
  }
}

function fileViewer(c: Ctx, path: string, text: string | null, lines: WeftBlameLine[] | null): HTMLElement {
  const blame = btn(c.blame ? 'Hide blame' : 'Blame', () => {
    c.blame = !c.blame;
    c.seen = '';
    void draw(c, true);
  }, { kind: 'ghost', sm: true, pressed: c.blame });
  const bar = h('div.hst2-vhead', null, h('code', null, path), blame);
  if (text === null) {
    return h('div', null, bar, h('p.hint', null, 'This file is not text, or it could not be read.'));
  }
  const rows = text.split('\n');
  if (rows.length && rows[rows.length - 1] === '') rows.pop();
  // Blame runs: the first line of each run carries its label; the runs alternate shade so the reader sees the seams.
  const labelAt = new Map<number, string>();
  const runOf = new Map<number, number>();
  if (c.blame && lines) {
    blameRuns(lines).forEach((run, i) => {
      labelAt.set(run.from, `${run.change} · ${nameOfCid(run.cid, run.actorKind)}${run.model ? ` · ${run.model}` : ''}`);
      for (let n = run.from; n <= run.to; n++) runOf.set(n, i);
    });
  }
  const shown = rows.slice(0, MAX_LINES);
  return h(
    'div',
    null,
    bar,
    h(
      'div.hst2-code',
      { role: 'region', 'aria-label': `${path} as text` },
      shown.map((t, i) => {
        const n = i + 1;
        const run = runOf.get(n);
        return h(
          'div.hst2-line',
          { class: run !== undefined ? (run % 2 ? 'hst2-run-b' : 'hst2-run-a') : null },
          c.blame && lines ? h('span.hst2-gut', null, labelAt.get(n) ?? '') : null,
          h('span.hst2-ln', null, String(n)),
          h('span.hst2-tx', null, t || ' '),
        );
      }),
    ),
    rows.length > MAX_LINES ? h('p.hint', null, `Showing the first ${MAX_LINES} of ${rows.length} lines.`) : null,
  );
}

// ---- One change --------------------------------------------------------------

async function changeView(c: Ctx, repoName: string, name: string): Promise<Built> {
  const change = await weft.change(name);
  const rev = latestRevision(change);
  // The signature check walks the chain: it is kept until the change gains a record.
  const [verify, tree, diff] = await Promise.all([
    cached(c, `verify|${change.change}|${change.records.length}`, () => weft.verify(change.change)).catch(() => null),
    c.expert && rev ? cached(c, `tree|${rev}`, () => weft.tree(rev)) : Promise.resolve(null),
    c.expert && rev ? cached(c, `diff|${rev}`, () => weft.diff(rev)) : Promise.resolve(null),
  ]);
  const status = changeStatus(change);
  const rounds = Object.keys(change.intents).map(Number);
  const intents: WeftIntent[] = change.intents[String(Math.max(0, ...rounds))] ?? [];
  const verified = verify ? (verify.ok ? badge('Signatures checked', 'ok') : badge('Signature check failed', 'danger')) : null;

  const meta = h(
    'p.hx-faint',
    null,
    h('code', null, change.change),
    change.task ? ` · task ${change.task}` : null,
    ` · ${change.import ? change.import.author_name : nameOfLabel(change.author)}`,
    change.import ? h('span', null, ' · imported from git ', h('code', { title: change.import.git }, change.import.git.slice(0, 7))) : null,
    change.stacked_on ? ` · stacked on ${change.stacked_on}` : null,
    ' ',
    badge(changeStateLabel(change.state), changeStateTone(change.state)),
    verified ? ' ' : null,
    verified,
  );
  const parts: (Node | null)[] = [
    h('a.hx-back', { href: href('history', repoName, 'changes') }, `← ${repoName}`),
    h('div.view-head.hx-trow', null, h('h1', null, change.title), h('div', null, expertButton(c))),
    meta,
    status.needsYou
      ? statusLine('is-needs is-ruling', lu('shield-check', 16), status.text)
      : h('p.hx-status', null, status.text),
  ];
  const cards: HTMLElement[] = [
    panel(
      'Sign-off',
      change.import && !change.records.length
        ? h('p.hx-faint', null, 'Imported from git: written before Weft, so it has no sign-off.')
        : signOffView(signOffChain(change.records), nameOfLabel),
    ),
    panel('Intents', intentsView(intents)),
  ];
  if (c.expert) {
    cards.push(
      panel('Revisions', revisionsView(change.revisions)),
      panel('Records', recordsView(change.records)),
      panel('Change tree', treeView(tree)),
      panel('Diff', diffView(diff)),
    );
  }
  parts.push(h('div.stack', null, ...cards));
  return {
    sig: JSON.stringify([change, verify?.ok ?? null, c.expert, tree, diff]),
    node: wrap(...parts),
  };
}

function expertButton(c: Ctx): HTMLButtonElement {
  return btn(
    c.expert ? 'Expert: on' : 'Expert',
    () => {
      c.expert = !c.expert;
      writeExpert(c.expert);
      c.seen = '';
      void draw(c, true);
    },
    { kind: 'hx-expert', pressed: c.expert, title: 'The dense view: revisions, every record, the change tree and the diff' },
  );
}

function intentsView(list: WeftIntent[]): HTMLElement {
  if (!list.length) return h('p.hx-faint', null, 'No intents yet.');
  return h(
    'div',
    null,
    list.map((i) =>
      h(
        'div',
        { class: 'hx-intent' },
        h('span.hx-chip', null, h('b', null, i.kind)),
        ' ',
        h('b', null, i.title),
        i.summary ? h('p.hx-faint', null, i.summary) : null,
        i.covers && i.covers.length ? h('p.hx-faint', null, 'covers ', ...i.covers.flatMap((p) => [h('code', null, p), ' '])) : null,
      ),
    ),
  );
}

function revisionsView(rows: WeftRevisionRow[]): HTMLElement {
  if (!rows.length) return h('p.hx-faint', null, 'No revisions.');
  return h(
    'ol',
    { class: 'hx-list' },
    rows.map((r) =>
      h(
        'li',
        null,
        h('code', null, r.id),
        h('span.hx-faint', null, ` ${r.kind}`),
        r.turn !== null ? h('span.hx-faint', null, ` · turn ${r.turn}`) : null,
        r.model ? h('span', null, ' · ', h('code', null, r.model)) : null,
        h('span.hx-faint', null, ` · ${plural(r.tool_calls.length, 'tool call')} · ${plural(r.files, 'file')} · ${nameOfLabel(r.actor)}`),
      ),
    ),
  );
}

function recordsView(records: WeftRecord[]): HTMLElement {
  if (!records.length) return h('p.hx-faint', null, 'No records.');
  return h(
    'div',
    null,
    [...records]
      .sort((a, b) => a.seq - b.seq)
      .map((r) =>
        h(
          'details',
          { class: 'hx-rec' },
          h(
            'summary',
            null,
            h('b', null, r.type),
            ' ',
            h('span.hx-faint', null, h('code', null, r.revision), ` · ${nameOfLabel(r.actor)} · ${ago(r.at_ms)}`),
            r.via_console ? ` ${badge('from the console', 'accent')}` : null,
          ),
          h('p.hx-faint', null, 'key ', h('code', { title: r.key }, shortKey(r.key)), ` · seq ${r.seq}`),
          h('pre', null, JSON.stringify(r.predicate, null, 2)),
        ),
      ),
  );
}

function treeView(tree: WeftChangeTree | null): HTMLElement {
  if (!tree) return h('p.hx-faint', null, 'No tree for this change yet.');
  const langs = Object.entries(tree.languages)
    .map(([k, v]) => `${k} ${v}`)
    .join(', ');
  return h(
    'div',
    null,
    h('p.hx-faint', null, `+${tree.add} −${tree.del} · ${plural(tree.files.length, 'file')}${langs ? ` · ${langs}` : ''}`),
    tree.files.map((f) =>
      h(
        'details',
        { class: 'hx-tree-f', open: true },
        h('summary', null, badge(f.op, OP_TONE[f.op] ?? 'muted'), ' ', h('code', null, f.path), h('span.hx-faint', null, ` +${f.add} −${f.del}`)),
        f.symbols.map((s) => symbolItem(s)),
      ),
    ),
  );
}

function symbolItem(s: WeftSymbol): HTMLElement {
  const who = [...new Set(s.provenance.map((p) => [nameOfActor(p.by), p.model, p.turn !== null ? `turn ${p.turn}` : null].filter(Boolean).join(' · ')))].join('; ');
  const touches = s.touches.map((t) => `${t.how} ${t.to}`).join('; ');
  return h(
    'div',
    { class: 'hx-sym' },
    h(
      'span',
      null,
      h('span', { title: OP_WORD[s.op] ?? s.op }, badge(s.op, OP_TONE[s.op] ?? 'muted')),
      ' ',
      h('code', null, s.qualified),
      h('span.hx-faint', null, ` ${s.kind} · lines ${s.start_line}–${s.end_line}`),
    ),
    who ? h('span.hx-faint', null, `by ${who}`) : null,
    touches ? h('span.hx-faint', null, touches) : null,
  );
}

function diffView(diff: WeftDiff | null): HTMLElement {
  if (!diff) return h('p.hx-faint', null, 'No diff for this change yet.');
  if (!diff.files.length) return h('p.hx-faint', null, 'No changes.');
  return h(
    'div',
    null,
    diff.files.map((f) =>
      h(
        'details',
        { class: 'hx-dfile' },
        h('summary', { class: 'dfile-h' }, h('code', null, f.path), h('span.hx-faint', null, ` +${f.add} −${f.del}`)),
        f.hunks.map((hk) =>
          h(
            'div',
            { class: 'diff' },
            h('div.hx-faint', null, h('code', null, `@@ -${hk.old_start},${hk.old_lines} +${hk.new_start},${hk.new_lines} @@`)),
            hk.lines.map(([op, text]) => h('div', { class: `dl ${diffClass(op) === 'add' ? 'add' : diffClass(op) === 'del' ? 'del' : ''}` }, h('span.hx-op', null, op), h('span', null, text))),
          ),
        ),
      ),
    ),
  );
}

// ---- The other repository tabs: Queue, Trace, Keys, Sync and Ops (the prototype's m-weft-more.js) -----------------------
// Each reads what the hub keeps. Left out, because the hub has no route for them: pausing the landing line, the clash
// view, adding and revoking keys, the import dry run, and the review verdict of a traced run.

const MIN = 60_000;

/** The body of one repository tab. */
async function tabBody(c: Ctx, repo: WeftRepo, tab: WeftTab): Promise<Built> {
  switch (tab) {
    case 'changes':
      return changesBody(c, repo);
    case 'queue':
      return queueBody(repo);
    case 'releases':
      return releasesBody(c, repo);
    case 'map':
      return mapBody(c, repo);
    case 'files':
      return filesBody(c, repo);
    case 'trace':
      return traceBody(c, repo);
    case 'keys':
      return keysBody(c);
    case 'sync':
      return syncBody(c, repo);
    case 'ops':
      return opsBody(c);
  }
}

/** A table in the prototype's markup: `div.tbl-wrap > table.tbl`. `cls` gives a column's class, `row` a row's. */
type Cell = Node | string | (Node | string)[];

function grid(heads: string[], rows: Cell[][], o: { cls?: (string | null)[]; row?: (i: number) => string | null } = {}): HTMLElement {
  return h(
    'div.tbl-wrap',
    null,
    h(
      'table.tbl',
      null,
      h('thead', null, h('tr', null, heads.map((x) => h('th', null, x)))),
      h(
        'tbody',
        null,
        rows.map((cells, i) => h('tr', { class: o.row?.(i) ?? null }, cells.map((cell, j) => h('td', { class: o.cls?.[j] ?? null }, cell)))),
      ),
    ),
  );
}

/** A labelled field whose class lets it grow in a row of fields (the prototype's `wmr-field`). */
function rowField(label: string, control: Node, hint?: string): HTMLElement {
  const el = field(label, control as HTMLElement, hint);
  el.classList.add('wmr-field');
  return el;
}

/** A wait as the queue shows it: under a minute, `N min`, or `N h M min`. */
function waitText(ms: number): string {
  const m = Math.max(0, Math.round(ms / MIN));
  if (m < 1) return 'under a minute';
  if (m < 60) return `${m} min`;
  return `${Math.floor(m / 60)} h${m % 60 ? ` ${m % 60} min` : ''}`;
}

/** A hub actor as a name: the owner and the hub keep their words, an agent is the agent's name. */
function actorName(a: WeftActor): string {
  if (a.kind === 'owner') return 'Owner';
  if (a.kind === 'hub') return 'Hub';
  return nameOfActor(a);
}

// ---- Queue -----------------------------------------------------------------------------------------------------------

const QUEUE_STATE: Record<string, [string, Tone]> = { queued: ['Queued', 'info'], landing: ['Landing', 'accent'], conflict: ['Conflict', 'danger'] };

/** The landing line onto main: the changes in order, the head that moves main. */
async function queueBody(repo: WeftRepo): Promise<Built> {
  const q = await weft.queue(repo.name);
  const n = q.queue.length;
  const bar = h(
    'div.wmr-bar',
    null,
    h('span.wmr-line', null, n ? `${plural(n, 'change')} in line for main. Only the head moves main.` : 'Nothing is in line for main.'),
    h('span.wmr-grow'),
    h('span.wmr-line', null, 'main is at ', h('code', null, q.head || 'root')),
  );
  const rows = q.queue.map((x, i) => {
    const [label, tone] = QUEUE_STATE[x.state] ?? [x.state, 'muted' as Tone];
    return [
      String(i + 1),
      h('div', null, h('code', null, x.change), h('div.wmr-small', null, x.title)),
      x.task ?? '—',
      x.author,
      badge(label, tone, true),
      waitText(Date.now() - x.at_ms),
    ];
  });
  const table = n
    ? grid(['#', 'Change', 'Task', 'Agent', 'State', 'Waiting'], rows, {
        cls: ['num', null, null, null, null, 'wmr-nowrap'],
        row: (i) => `wmr-q is-${q.queue[i]?.state ?? ''}`,
      })
    : empty('git', 'Nothing is in line', 'Changes join the queue when they are ready to land.');
  const hint = h('p.wmr-hint', null, 'Only the head moves main. A change that shares no file with the head may land first. A clash leaves the line: rebase it onto main, or send it back to its agent.');
  return { sig: JSON.stringify([q, Math.floor(Date.now() / MIN)]), node: h('div.wmr-stack', null, panel('Landing queue', [bar, table, hint])) };
}

// ---- Keys and the verify panel ----------------------------------------------------------------------------------------

const KEY_KIND: Record<string, [string, Tone]> = { owner: ['Owner', 'accent'], hub: ['Hub', 'info'], agent: ['Agent', 'muted'] };

/** The verify panel's subject and its last answer (it is asked on demand, not on every poll). */
const verifyUi: { subject: string; result: WeftVerify | null } = { subject: '', result: null };

/** What the hub signed under a subject, record by record, each walked to the owner key. */
function verifyView(v: WeftVerify): Node[] {
  const head = h(
    'p.wmr-line',
    null,
    badge(v.ok ? 'Signature checks out' : 'Signature check failed', v.ok ? 'ok' : 'danger', true),
    ' ',
    h('span', null, `${v.subject}: ${plural(v.records.length, 'signed record')}, each walked to the owner key.`),
  );
  const rows = v.records.map((r) => [
    String(r.seq),
    r.kind,
    r.actor === 'owner' ? 'Owner' : r.actor === 'hub' ? 'Hub' : nameOfLabel(r.actor),
    h('code', null, r.key.slice(0, 8)),
    h('span.wmr-chain', null, r.chain.join(' → ')),
    r.via_console ? 'Yes' : 'No',
    r.ok ? badge('Ok', 'ok', true) : badge('Failed', 'danger', true),
  ]);
  return [head, grid(['Seq', 'Kind', 'Signed by', 'Key', 'Chain to owner key', 'Via console', 'Result'], rows, { cls: ['num'] })];
}

async function runVerify(c: Ctx): Promise<void> {
  const subject = verifyUi.subject.trim();
  if (!subject) {
    toast('Name what to verify: a change, a revision or a release.', 'error');
    return;
  }
  try {
    const result = await weft.verify(subject);
    verifyUi.result = result;
    toast(result.ok ? `${subject} checks out` : `${subject}: the signature check failed`, result.ok ? 'info' : 'error');
  } catch (err) {
    verifyUi.result = null;
    toast(errMsg(err), 'error');
  }
  c.seen = '';
  void draw(c, true);
}

/** The hub's signing keys (the owner's, the hub's, the agents') and the verify panel under them. */
async function keysBody(c: Ctx): Promise<Built> {
  const k = await weft.keys();
  const rows = k.keys.map((x) => {
    const [kind, tone] = KEY_KIND[x.holder.actor.kind] ?? [x.holder.actor.kind, 'muted' as Tone];
    return [
      h('div', null, h('b', null, x.holder.name || actorName(x.holder.actor)), h('div.wmr-small', null, badge(kind, tone))),
      h('code', { title: x.id }, x.id.slice(0, 12)),
      h('span.wmr-clip', { title: x.key }, x.key),
      `record ${x.issued_seq}`,
      x.caps.join(', ') || '—',
      x.revoked_seq !== null ? badge('Revoked', 'danger', true) : badge('Active', 'ok', true),
    ];
  });
  const keys = panel('Signing keys', [
    h('p.wmr-desc', null, 'Ed25519 keys, each named by its id. A revoked key signs nothing new, and the records it signed stay as they were.'),
    k.keys.length
      ? grid(['Holder', 'Id', 'Key', 'Issued', 'Signs', 'Status'], rows, { cls: [null, null, null, 'wmr-nowrap', null, null], row: (i) => (k.keys[i]?.revoked_seq != null ? 'is-revoked' : null) })
      : empty('git', 'No keys yet', 'Signing keys show here once this repository has signed records.'),
  ]);
  const subject = h('input.input', {
    value: verifyUi.subject,
    placeholder: 'w-212, w-206.r1 or acme-web@v0.3.0',
    autocomplete: 'off',
    spellcheck: 'false',
    'aria-label': 'Subject to verify',
    oninput: (e: Event) => {
      verifyUi.subject = (e.target as HTMLInputElement).value;
    },
  });
  const verify = panel('Verify a signature', [
    h('div.wmr-row-in', null, rowField('Subject', subject, 'A change, a revision or a release.'), btn('Verify', () => void runVerify(c), { kind: 'primary' })),
    ...(verifyUi.result ? verifyView(verifyUi.result) : []),
  ]);
  return { sig: JSON.stringify([k, verifyUi.result, verifyUi.subject]), node: h('div.wmr-stack', null, keys, verify) };
}

// ---- Ops -------------------------------------------------------------------------------------------------------------

const OP_GROUPS: [string, string][] = [
  ['all', 'All'],
  ['land', 'Land'],
  ['seal', 'Seal'],
  ['sign', 'Sign'],
  ['export', 'Export'],
  ['import', 'Import'],
  ['adopt', 'Adopt'],
  ['revert', 'Revert'],
  ['queue', 'Queue'],
  ['key', 'Key'],
];
const OPS_TONE: Record<string, Tone> = { land: 'ok', seal: 'info', sign: 'accent', export: 'info', import: 'info', adopt: 'warn', revert: 'warn', queue: 'muted', key: 'muted', other: 'muted' };
const opsUi: { op: string } = { op: 'all' };

/** The group an operation's kind belongs to (`revision.seal` is a seal, `cert.issue` a key); `other` when none. */
function opGroup(kind: string): string {
  if (kind.startsWith('revision.seal')) return 'seal';
  if (kind.startsWith('release.sign')) return 'sign';
  if (kind.includes('export')) return 'export';
  if (kind.includes('import')) return 'import';
  if (kind.includes('adopt')) return 'adopt';
  if (kind.includes('revert')) return 'revert';
  if (kind.startsWith('queue.')) return 'queue';
  if (kind.startsWith('cert.') || kind.startsWith('key')) return 'key';
  if (kind.includes('land')) return 'land';
  return 'other';
}

/** The hub's operations log, newest first, filtered by its group. */
async function opsBody(c: Ctx): Promise<Built> {
  const ops = await weft.ops(200);
  const list = [...ops].reverse().filter((o) => opsUi.op === 'all' || opGroup(o.kind) === opsUi.op);
  const seg = h(
    'div.seg.wmr-seg',
    { role: 'group', 'aria-label': 'Filter by operation' },
    OP_GROUPS.map(([key, label]) =>
      h(
        'button',
        {
          type: 'button',
          class: opsUi.op === key ? 'on' : null,
          'aria-pressed': String(opsUi.op === key),
          onclick: () => {
            opsUi.op = key;
            c.seen = '';
            void draw(c, true);
          },
        },
        label,
      ),
    ),
  );
  const rows = list.map((o) => [
    h('span', { title: new Date(o.at_ms).toLocaleString() }, ago(o.at_ms)),
    actorName(o.actor),
    badge(o.kind, OPS_TONE[opGroup(o.kind)] ?? 'muted'),
    o.summary,
  ]);
  const table = list.length
    ? grid(['When', 'Actor', 'Op', 'Detail'], rows, { cls: ['wmr-nowrap', null, null, null] })
    : empty('git', 'Nothing logged for this op', 'Pick All to see every operation.');
  return { sig: JSON.stringify([ops.length, ops.at(-1)?.seq ?? 0, opsUi.op]), node: h('div.wmr-stack', null, panel('Operations log', [seg, table])) };
}

// ---- Sync: the connections to outside git, the export with its dry run, the adoption of a moved remote ---------------

const syncUi: { conn: string; dry: { repo: string; conn: string; report: WeftExport } | null } = { conn: '', dry: null };

/** What a dry run of an export would push: the changes and their commits, nothing written. */
function dryView(r: WeftExport): HTMLElement {
  const n = r.changes.length;
  return h(
    'div.wmr-dry',
    null,
    h(
      'p.wmr-line',
      null,
      badge('Dry run', 'info', true),
      ' ',
      h('span', null, n ? `Would push ${plural(n, 'change')} to ${r.remote}. Nothing is pushed by a dry run.` : `Nothing to push: ${r.remote} is in step with main.`),
    ),
    n
      ? h(
          'ul.wmr-list',
          null,
          r.changes.map((ch) =>
            h(
              'li',
              null,
              h('code', null, ch.change),
              ' ',
              ch.commits.map((m) => h('span.wmr-small', null, `${m.intent} · `, h('code', null, m.git), ` · ${plural(m.files, 'file')}`)),
            ),
          ),
        )
      : null,
  );
}

async function dryRunExport(c: Ctx, repo: string): Promise<void> {
  const conn = syncUi.conn;
  if (!conn) {
    toast('Pick a connection first.', 'error');
    return;
  }
  try {
    syncUi.dry = { repo, conn, report: await weft.exportRepo(repo, conn, true) };
    toast(`Dry run for ${conn}. Nothing was pushed.`);
  } catch (err) {
    syncUi.dry = null;
    toast(errMsg(err), 'error');
  }
  c.seen = '';
  void draw(c, true);
}

async function exportNow(c: Ctx, repo: string): Promise<void> {
  const d = syncUi.dry;
  if (!d || d.repo !== repo || d.conn !== syncUi.conn) {
    toast('Run a dry run first, so you see what would be pushed.', 'error');
    return;
  }
  const n = d.report.changes.length;
  if (!n) {
    toast(`Nothing to export: ${d.report.remote} is in step with main.`);
    return;
  }
  syncUi.dry = null;
  await act(c, () => weft.exportRepo(repo, d.conn), `Exported ${plural(n, 'change')} to ${d.report.remote}`);
}

/** Asks before a moved remote's own commits come into main. */
function askAdopt(c: Ctx, repo: string, x: WeftConnection): void {
  const close = modal(
    `Adopt ${x.id}?`,
    h('p.wmr-desc', null, `${x.id} moved on its own. Adopting brings its commits into main as imported changes, with their authors and dates kept.`),
    undefined,
    [
      btn('Cancel', () => close(), { kind: 'ghost' }),
      btn(
        'Adopt',
        () => {
          close();
          void act(c, () => weft.adopt(repo, x.id), `Adopting the commits of ${x.id}`);
        },
        { kind: 'primary' },
      ),
    ],
  );
}

function syncConnRow(c: Ctx, repo: WeftRepo, x: WeftConnection): HTMLElement {
  const action =
    x.state === 'in-step'
      ? btn('Dry run export', () => {
          syncUi.conn = x.id;
          void dryRunExport(c, repo.name);
        }, { kind: 'ghost' })
      : x.state === 'moved'
        ? btn('Adopt', () => askAdopt(c, repo.name, x), { kind: 'primary' })
        : null;
  return h(
    'li.wmr-conn',
    null,
    h(
      'div.wmr-conn-main',
      null,
      h('div.wmr-conn-t', null, badge(connectionLabel(x.state), connectionTone(x.state), true), h('b', null, x.id), h('code', null, x.remote)),
      h('p.wmr-small', null, x.detail),
    ),
    h('div.wmr-acts', null, action),
  );
}

async function syncBody(c: Ctx, repo: WeftRepo): Promise<Built> {
  const conns = repo.connections;
  if (!conns.some((x) => x.id === syncUi.conn)) syncUi.conn = conns[0]?.id ?? '';
  const connections = panel('Connections to outside git', [
    h('p.wmr-desc', null, 'Where this repository meets outside git. A connection that moved on its own is adopted, never overwritten.'),
    conns.length ? h('ul.wmr-conns', null, conns.map((x) => syncConnRow(c, repo, x))) : h('p.wmr-faint', null, 'No outside git connections yet.'),
  ]);
  const select = h(
    'select.select',
    {
      'aria-label': 'Connection',
      onchange: (e: Event) => {
        syncUi.conn = (e.target as HTMLSelectElement).value;
        syncUi.dry = null;
        c.seen = '';
        void draw(c, true);
      },
    },
    conns.map((x) => h('option', { value: x.id, selected: x.id === syncUi.conn }, x.id)),
  );
  const dry = syncUi.dry && syncUi.dry.repo === repo.name && syncUi.dry.conn === syncUi.conn ? syncUi.dry.report : null;
  const exportPanel = panel('Export to git', [
    h('p.wmr-desc', null, "One commit per intent, authored by the owner and signed with the hub's key."),
    h(
      'div.wmr-row-in',
      null,
      rowField('Connection', select, 'Export is fast-forward only: a connection that moved on its own is not pushed to.'),
      btn('Dry run', () => void dryRunExport(c, repo.name), { kind: 'ghost' }),
      btn('Export', () => void exportNow(c, repo.name), { kind: 'primary' }),
    ),
    ...(dry ? [dryView(dry)] : []),
  ]);
  return { sig: JSON.stringify([repo.connections, syncUi.conn, dry]), node: h('div.wmr-stack', null, connections, exportPanel) };
}

// ---- Trace: which agent wrote each run of lines in a file, in which change ----------------------------------------------

const traceUi: { file: string } = { file: '' };

/** The file's lines at the head, in runs: who wrote them, in which change and revision, and the turn that wrote them. */
async function traceBody(c: Ctx, repo: WeftRepo): Promise<Built> {
  const none = (): Node => panel('Agent trace', empty('git', 'No traced files', 'Agent trace shows once a repository has agent-written lines.'));
  const head = headOf(repo);
  const listing = await cached(c, `files|${repo.name}|${head}`, () => weft.files(head)).catch(() => null);
  const paths = listing?.files.map((f) => f.path) ?? [];
  if (!paths.length) return { sig: 'none', node: h('div.wmr-stack', null, none()) };
  if (!paths.includes(traceUi.file)) traceUi.file = paths[0] ?? '';
  const file = traceUi.file;
  const blame = await cached(c, `blame|${repo.name}|${head}|${file}`, () => weft.blame(repo.name, file)).catch(() => null);
  const runs = blame ? blameRuns(blame.lines) : [];
  const select = h(
    'select.select',
    {
      'aria-label': 'File',
      onchange: (e: Event) => {
        traceUi.file = (e.target as HTMLSelectElement).value;
        c.seen = '';
        void draw(c, true);
      },
    },
    paths.map((p) => h('option', { value: p, selected: p === file }, p)),
  );
  const rows = runs.map((r) => [
    `L${r.from}–${r.to}`,
    r.actorKind === 'owner'
      ? [h('b', null, 'Owner'), ' ', badge('human', 'muted')]
      : r.actorKind === 'agent'
        ? [h('b', null, nameOfCid(r.cid, 'agent')), ' ', badge(r.model ? `ai · ${r.model}` : 'ai', 'accent')]
        : [h('b', null, r.actorKind), ' ', badge(r.actorKind, 'muted')],
    h('code', null, r.change),
    h('code', null, r.source),
    r.turn != null ? `turn ${r.turn}` : '—',
  ]);
  const table = runs.length
    ? grid(['Lines', 'Written by', 'Change', 'Revision', 'Turn'], rows, { cls: ['wmr-nowrap', null, null, null, 'wmr-nowrap'] })
    : h('p.wmr-faint', null, 'No lines in this file.');
  const top = h('div.wmr-row-in', null, rowField('File', select, `${plural(runs.length, 'run')} in this file, in line order.`));
  return { sig: JSON.stringify([file, runs]), node: h('div.wmr-stack', null, panel('Agent trace', [top, table])) };
}
