// History: the plaza's version control (Weft) as the owner reads it. The repositories, each with its changes as the
// line of main and the open lanes beside it; a change, with its sign-off chain and (in Expert) its revisions, records,
// tree and diff; the releases (the owner signs and exports them here); the map; and the files at main with blame.
// Read by polling every few seconds: the live bus does not carry the weft events, and each read is cheap.

import '../styles/history.css';
import {
  weft,
  type WeftActor,
  type WeftArtifact,
  type WeftBlameLine,
  type WeftChangeTree,
  type WeftComponent,
  type WeftConnection,
  type WeftDiff,
  type WeftIntent,
  type WeftRecord,
  type WeftRelease,
  type WeftRepo,
  type WeftRevisionRow,
  type WeftSymbol,
  type WeftWhereHit,
} from '../api-weft';
import { agentName } from '../roles';
import { href } from '../router';
import { projectTitle, scope } from '../scope';
import { agents } from '../store';
import { ago, attempt, badge, button, card, clip, empty, input, select, tabs, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import { laneGraph, signOffView } from './history-graph';
import {
  HISTORY_TABS,
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
  parseHistoryRoute,
  plural,
  presetKey,
  releaseStatus,
  repoLine,
  scopedRepos,
  shortKey,
  signOffChain,
  sizeText,
  sortReleases,
  splitLanes,
  type FileNode,
  type HistoryRoute,
  type HistoryTab,
} from './history-model';
import type { Cleanup } from './index';

/** How often the screen rereads what it shows. */
const POLL_MS = 5_000;
/** The most lines a file shows; a longer one says how many it has. */
const MAX_LINES = 4000;
const EXPERT_KEY = 'apz.history.expert';

/** One drawn screen: its markup and the signature of the data it was drawn from (a poll redraws only on a change). */
interface Built {
  sig: string;
  node: HTMLElement;
}

interface Ctx {
  root: HTMLElement;
  route: HistoryRoute;
  alive: boolean;
  /** An action is in flight: polls wait for it. */
  busy: boolean;
  gen: number;
  seen: string;
  /** The changes query as committed, and as typed so far (a poll must not lose the typing). */
  query: string;
  draft: string;
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

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const c: Ctx = {
    root,
    route: parseHistoryRoute(params),
    alive: true,
    busy: false,
    gen: 0,
    seen: '',
    query: '',
    draft: '',
    expert: readExpert(),
    file: '',
    blame: false,
    where: null,
    whereDraft: '',
    cache: new Map(),
  };
  replace(root, wrap(pageHead('History'), h('p.hx-faint', null, 'Loading…')));
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
    built = { sig: `error:${errMsg(err)}`, node: wrap(pageHead('History'), empty('git', 'Weft could not be read', errMsg(err))) };
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
  return agent ? agentName(agent) : `agent ${cid}`;
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

function pageHead(title: string, ...actions: Node[]): HTMLElement {
  return h('div.view-head', null, h('h1', null, title), actions.length ? h('div.hx-head-actions', null, ...actions) : null);
}

/** The defined children of a card or a row, in order (a null stands for nothing). */
function list(...items: (Node | null | undefined | false)[]): Node[] {
  return items.filter((x): x is Node => !!x);
}

function lineEl(text: string, needsYou: boolean): HTMLElement {
  return h('p.hx-line', { class: needsYou ? 'needs' : null }, text);
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
    return { sig: 'off', node: wrap(pageHead('History'), empty('git', 'Weft is not running', 'Start it on the hub to see the plaza’s repositories.')) };
  }
  const repos = scopedRepos(await weft.repos(), scope.get());
  const waiting = await Promise.all(repos.map((r) => weft.log(r.name, 'needs(owner)', 200).then((l) => l.total).catch(() => 0)));
  const total = waiting.reduce((a, b) => a + b, 0);
  // Proposed releases wait for the owner's signature (imported ones never do).
  const unsigned = repos.reduce((n, r) => n + (r.releases ?? []).filter((x) => !x.signed && !x.imported).length, 0);
  const line = total
    ? `${plural(total, 'change')} waiting for your ruling.`
    : unsigned
      ? `${plural(unsigned, 'release')} waiting for your signature.`
      : repos.length
        ? `${plural(repos.length, 'repository')}. Nothing waits for you.`
        : 'No repositories yet.';
  const body = repos.length
    ? h(
        'div.hx-repos',
        null,
        repos.map((r, i) => repoCard(r, waiting[i] ?? 0)),
      )
    : empty('git', 'No repositories yet', 'A project’s repository shows here once it has one.');
  return { sig: JSON.stringify([repos, waiting]), node: wrap(pageHead('History'), lineEl(line, total + unsigned > 0), body) };
}

function repoCard(r: WeftRepo, waiting: number): HTMLElement {
  return h(
    'a.hx-repo',
    { href: href('history', r.name) },
    h('span.hx-repo-top', null, h('h2', null, r.name), r.project ? h('span.hx-faint', null, projectTitle(r.project)) : null),
    r.description ? h('p.hx-desc', null, r.description) : null,
    h('p.hx-repo-head', null, 'main at ', h('code', null, headOf(r))),
    h('p.hx-counts', null, `${plural(r.landed, 'landed change')} · ${r.open_changes} open`),
    r.connections.length ? h('span.hx-conns', null, r.connections.map(connBadge)) : null,
    ...r.connections.filter((x) => x.state !== 'in-step' && x.detail).map((x) => h('p.hx-faint', null, `${x.id}: ${x.detail}`)),
    waiting ? badge(`${waiting} waiting for you`, 'warn') : null,
  );
}

// ---- One repository ----------------------------------------------------------

async function repoView(c: Ctx, name: string, tab: HistoryTab): Promise<Built> {
  const [repo, waiting] = await Promise.all([weft.repo(name), weft.log(name, 'needs(owner)', 200)]);
  const line = repoLine(repo, waiting.total);
  const body =
    tab === 'changes'
      ? await changesBody(c, repo)
      : tab === 'releases'
        ? await releasesBody(c, repo)
        : tab === 'map'
          ? await mapBody(c, repo)
          : await filesBody(c, repo);
  const node = wrap(
    h(
      'div.view-head',
      null,
      h('div.hx-titlebox', null, h('a.hx-back', { href: href('history') }, '← Repositories'), h('h1', null, repo.name)),
    ),
    repo.description ? h('p.hx-desc', null, repo.description) : null,
    lineEl(line.text, line.needsYou),
    tabs(HISTORY_TABS, tab, (key) => {
      location.hash = href('history', name, key);
    }),
    body.node,
  );
  return { sig: JSON.stringify([repo, waiting.total, tab, body.sig]), node };
}

// Changes: the lane graph, with the query box and its presets above it.

async function changesBody(c: Ctx, repo: WeftRepo): Promise<Built> {
  const list = await weft.log(repo.name, c.query.trim() || 'all()', 200);
  const { main, lanes } = splitLanes(list.changes);
  const active = presetKey(c.query);
  const q = input({ type: 'search', placeholder: 'Filter, e.g. lane(w-2) | needs(owner)', value: c.draft, 'aria-label': 'Query' });
  q.addEventListener('input', () => {
    c.draft = q.value;
  });
  q.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter') return;
    c.query = c.draft.trim();
    c.seen = '';
    void draw(c, true);
  });
  const presets = h(
    'div.hx-presets',
    { role: 'group', 'aria-label': 'Quick filters' },
    QUERY_PRESETS.map((p) => {
      const b = button(p.label, () => {
        c.query = p.query;
        c.draft = p.query;
        c.seen = '';
        void draw(c, true);
      });
      b.setAttribute('aria-pressed', String(active === p.key));
      if (active === p.key) b.classList.add('on');
      return b;
    }),
  );
  const graph = laneGraph(repo.name, main, lanes, headOf(repo), nameOfLabel);
  return {
    sig: JSON.stringify([list.changes, c.query]),
    node: h('div.hx-changes', null, h('div.hx-bar', null, q, presets), graph),
  };
}

// Releases: the versions, their notes and artifacts; a proposed one is signed by the owner here, a signed one exported.

async function releasesBody(c: Ctx, repo: WeftRepo): Promise<Built> {
  const list = sortReleases(await weft.releases(repo.name));
  if (!list.length) {
    return { sig: 'none', node: empty('git', 'No releases yet', 'A release is proposed from a landed revision, then signed here.') };
  }
  const candidates = new Map<string, WeftArtifact[]>();
  await Promise.all(
    list
      .filter((r) => releaseStatus(r) === 'proposed')
      .map(async (r) => {
        candidates.set(r.version, await weft.artifacts({ revision: r.from }).catch(() => []));
      }),
  );
  const node = h(
    'div.hx-releases',
    null,
    list.map((r) => releaseCard(c, repo, r, candidates.get(r.version) ?? [])),
  );
  return { sig: JSON.stringify([list, [...candidates.entries()]]), node };
}

function releaseCard(c: Ctx, repo: WeftRepo, r: WeftRelease, candidates: WeftArtifact[]): HTMLElement {
  const status = releaseStatus(r);
  const artifacts = status === 'signed' || status === 'imported' ? r.artifacts : candidates;
  const ids = artifacts.map((a) => a.artifact.id);
  const connections = repo.connections;
  const signBtn =
    status === 'proposed'
      ? button(`Sign ${r.version}`, () => void signRelease(c, repo.name, r, ids), {
          kind: 'primary',
          title: 'Sign this release with your key: its artifacts and their digest',
        })
      : null;
  const exportArea = connections.length
    ? exportControl(c, repo.name, r, connections, status === 'signed')
    : h('p.hx-faint', null, 'No connection to export to yet.');
  const notes = r.notes.trim() ? h('div.hx-notes', null, renderMarkdown(r.notes)) : null;
  const artList = artifacts.length
    ? h(
        'ul.hx-artifacts',
        null,
        artifacts.map((a) => artifactRow(a)),
      )
    : h('p.hx-faint', null, status === 'proposed' ? 'No artifacts built from this revision yet.' : 'No artifacts.');
  const signedBy = r.signed ? h('p.hx-faint', null, `Signed by key ${shortKey(r.signed.key)} · digest ${shortKey(r.signed.digest)}`) : null;
  const exports = r.exports.length
    ? h(
        'ul.hx-exports',
        null,
        r.exports.map((e) =>
          h(
            'li',
            null,
            `Exported to ${e.connection} as ${e.tag}`,
            e.url && /^https?:\/\//.test(e.url) ? h('a', { href: e.url, target: '_blank', rel: 'noopener' }, ' Open') : null,
          ),
        ),
      )
    : null;
  return card(
    h('h2', null, r.version),
    list(
      h(
        'p.hx-release-meta',
        null,
        h('a', { href: href('history', repo.name, 'change', changeOfRevision(r.from)) }, `from ${r.from}`),
        r.line ? ` · line ${r.line}` : null,
      ),
      notes,
      h('h3.hx-sub', null, 'Artifacts'),
      artList,
      signedBy,
      h('div.hx-actions', null, signBtn, exportArea),
      exports,
    ),
    { actions: [badge(RELEASE_LABEL[status], RELEASE_TONE[status])] },
  );
}

function artifactRow(a: WeftArtifact): HTMLElement {
  const id = a.artifact.id;
  const meta = [a.artifact.target, sizeText(a.artifact.size)].filter(Boolean).join(' · ');
  return h(
    'li',
    { class: 'hx-artifact' },
    h('span.hx-art-name', null, a.artifact.name, a.artifact.version ? ` ${a.artifact.version}` : null),
    h('span.hx-faint', null, meta || null),
    a.artifact.reproduced ? badge('Reproduced', 'ok') : null,
    h(
      'span.hx-links',
      null,
      h('a', { href: weft.artifactContent(id), target: '_blank', rel: 'noopener' }, 'Download'),
      h('a', { href: weft.artifactProvenance(id), target: '_blank', rel: 'noopener' }, 'Provenance'),
    ),
  );
}

/** The export button for a release: enabled once the owner has signed it; with several connections, a choice first. */
function exportControl(c: Ctx, repo: string, r: WeftRelease, connections: WeftConnection[], signed: boolean): HTMLElement {
  const first = connections[0]?.id ?? '';
  const choice = connections.length > 1 ? select(connections.map((x) => ({ value: x.id, label: x.id })), first) : null;
  const label = (id: string) => `Export to ${id}`;
  const btn = button(label(first), () => void exportTo(c, repo, r, choice?.value ?? first), {
    kind: 'primary',
    disabled: !signed,
    title: signed ? undefined : 'Sign the release first',
  });
  if (choice) {
    choice.addEventListener('change', () => {
      btn.textContent = label(choice.value);
    });
  }
  return h('span.hx-export', null, choice, btn);
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
        'div.hx-comps',
        null,
        comps.map((x) => componentEl(x)),
      )
    : empty('file', 'No components yet', 'Components come from the files the repository holds.');
  const moved = movedConnection(m.connections);
  const connsEl = m.connections.length
    ? h(
        'ul.hx-conns-list',
        null,
        m.connections.map((x) => connectionRow(c, repo.name, x)),
      )
    : h('p.hx-faint', null, 'No connection. Export goes nowhere until one is added.');
  const node = h(
    'div.hx-stack',
    null,
    card('Components', list(m.drafted ? h('p.hx-faint', null, 'Drafted from the files on main. No map has landed yet.') : null, compsEl)),
    card('Connections', list(moved ? h('p.hx-faint', null, `${moved.id} moved on its own. Adopt its commits to bring them in.`) : null, connsEl)),
    card('Where is…', list(whereBox(c, repo.name), whereHits(c))),
  );
  return { sig: JSON.stringify([m, c.where, c.whereDraft]), node };
}

function componentEl(x: WeftComponent): HTMLElement {
  const shown = x.paths.slice(0, 4);
  return h(
    'div.hx-comp',
    null,
    h('h3', null, x.title || x.name),
    x.title ? h('code.hx-comp-name', null, x.name) : null,
    h(
      'div.hx-paths',
      null,
      shown.map((p) => h('code', null, p)),
      x.paths.length > shown.length ? h('span.hx-faint', null, `+${x.paths.length - shown.length}`) : null,
    ),
    x.languages.length ? h('div.hx-chips', null, x.languages.map((l) => badge(l))) : null,
  );
}

function connectionRow(c: Ctx, repo: string, x: WeftConnection): HTMLElement {
  const adopt = x.state === 'moved' ? button('Adopt', () => void act(c, () => weft.adopt(repo, x.id), `Adopting the commits of ${x.id}`), { kind: 'primary' }) : null;
  return h(
    'li',
    { class: 'hx-conn' },
    h('div.hx-conn-top', null, h('strong', null, x.id), badge(connectionLabel(x.state), connectionTone(x.state)), h('span.hx-faint', null, `${x.platform} · ${x.branch}`)),
    h('code.hx-remote', null, clip(x.remote, 90)),
    x.detail ? h('p.hx-faint', null, x.detail) : null,
    h('div.hx-actions', null, adopt),
  );
}

function whereBox(c: Ctx, repo: string): HTMLElement {
  const q = input({ placeholder: 'Where is the queue?', value: c.whereDraft, 'aria-label': 'Where is' });
  q.addEventListener('input', () => {
    c.whereDraft = q.value;
  });
  const find = async () => {
    const question = c.whereDraft.trim();
    if (!question) return;
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
  return h('div.hx-where', null, q, button('Find', () => void find(), { kind: 'primary' }));
}

function whereHits(c: Ctx): HTMLElement | null {
  if (!c.where) return null;
  if (!c.where.hits.length) return h('p.hx-faint', null, `Nothing for “${c.where.q}”.`);
  return h(
    'ul.hx-hits',
    null,
    c.where.hits.map((hit) =>
      h(
        'li',
        null,
        h('code', null, hit.path),
        hit.symbol ? h('span', null, ` ${hit.symbol}`) : null,
        h('span.hx-faint', null, ` · ${hit.why}`),
        hit.component ? badge(hit.component) : null,
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
  let viewer: HTMLElement = h('p.hx-faint', null, 'Choose a file to read it.');
  let text: string | null = null;
  let lines: WeftBlameLine[] | null = null;
  if (c.file && !file) viewer = h('p.hx-faint', null, 'No such file on main.');
  if (file) {
    text = await readText(c, head, file.path);
    if (c.blame && text !== null) lines = await blameOf(c, repo.name, file.path, head);
    viewer = fileViewer(c, file.path, text, lines);
  }
  const sig = JSON.stringify([listing.files.map((f) => `${f.path}${f.hash}`), c.file, c.blame, text, lines]);
  return {
    sig,
    node: h('div.hx-files', null, h('aside.hx-tree-col', { 'aria-label': 'Files' }, treeNodes(c, tree, 0)), h('section.hx-viewer-col', null, viewer)),
  };
}

function treeNodes(c: Ctx, nodes: FileNode[], depth: number): HTMLElement {
  return h(
    'ul.hx-tree',
    null,
    nodes.map((n) => {
      if (n.dir) {
        return h('li', null, h('details', depth < 2 ? { open: true } : {}, h('summary', null, n.name), treeNodes(c, n.children, depth + 1)));
      }
      const on = c.file === n.path;
      return h(
        'li',
        null,
        h(
          'button.hx-file',
          { type: 'button', class: on ? 'on' : null, 'aria-current': on ? 'true' : null, onclick: () => openFile(c, n.path) },
          n.name,
          h('span.hx-size', null, sizeText(n.size)),
        ),
      );
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
  const blame = button(c.blame ? 'Hide blame' : 'Blame', () => {
    c.blame = !c.blame;
    c.seen = '';
    void draw(c, true);
  });
  blame.setAttribute('aria-pressed', String(c.blame));
  const bar = h('div.hx-viewer-head', null, h('code', null, path), blame);
  if (text === null) {
    return h('div.hx-viewer', null, bar, h('p.hx-faint', null, 'This file is not text, or it could not be read.'));
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
    'div.hx-viewer',
    null,
    bar,
    h(
      'div.hx-code',
      { class: c.blame ? 'with-blame' : null, role: 'region', 'aria-label': `${path} as text` },
      shown.map((t, i) => {
        const n = i + 1;
        const run = runOf.get(n);
        return h(
          'div.hx-code-line',
          { class: run !== undefined ? `run-${run % 2}` : null },
          c.blame ? h('span.hx-gutter', null, labelAt.get(n) ?? '') : null,
          h('span.hx-ln', null, String(n)),
          h('span.hx-src', null, t || ' '),
        );
      }),
    ),
    rows.length > MAX_LINES ? h('p.hx-faint', null, `Showing the first ${MAX_LINES} of ${rows.length} lines.`) : null,
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

  const parts: (Node | null)[] = [
    h(
      'div.view-head',
      null,
      h('div.hx-titlebox', null, h('a.hx-back', { href: href('history', repoName, 'changes') }, `← ${repoName}`), h('h1', null, change.title)),
      h('div.hx-head-actions', null, expertButton(c)),
    ),
    h(
      'p.hx-meta-line',
      null,
      h('code', null, change.change),
      change.task ? ` · task ${change.task}` : null,
      ` · ${change.import ? change.import.author_name : nameOfLabel(change.author)}`,
      change.import ? h('span.hx-faint', null, ' · imported from git ', h('code', { title: change.import.git }, change.import.git.slice(0, 7))) : null,
      change.stacked_on ? ` · stacked on ${change.stacked_on}` : null,
      ' ',
      badge(changeStateLabel(change.state), changeStateTone(change.state)),
      verified,
    ),
    lineEl(status.text, status.needsYou),
    card(
      'Sign-off',
      change.import && !change.records.length
        ? h('p.hx-faint', null, 'Imported from git: written before Weft, so it has no sign-off.')
        : signOffView(signOffChain(change.records), nameOfLabel),
    ),
    card('Intents', intentsView(intents)),
  ];
  if (c.expert) {
    parts.push(
      card('Revisions', revisionsView(change.revisions)),
      card('Records', recordsView(change.records)),
      card('Change tree', treeView(tree)),
      card('Diff', diffView(diff)),
    );
  }
  return {
    sig: JSON.stringify([change, verify?.ok ?? null, c.expert, tree, diff]),
    node: wrap(...parts),
  };
}

function expertButton(c: Ctx): HTMLButtonElement {
  const b = button(
    c.expert ? 'Expert: on' : 'Expert',
    () => {
      c.expert = !c.expert;
      writeExpert(c.expert);
      c.seen = '';
      void draw(c, true);
    },
    { title: 'The dense view: revisions, every record, the change tree and the diff' },
  );
  b.setAttribute('aria-pressed', String(c.expert));
  return b;
}

function intentsView(list: WeftIntent[]): HTMLElement {
  if (!list.length) return h('p.hx-faint', null, 'No intents yet.');
  return h(
    'ul.hx-intents',
    null,
    list.map((i) =>
      h(
        'li',
        { class: 'hx-intent' },
        h('div.hx-intent-top', null, h('span.hx-chip-kind', null, i.kind), h('strong', null, i.title)),
        i.summary ? h('p', null, i.summary) : null,
        i.covers && i.covers.length ? h('p.hx-covers', null, 'covers ', i.covers.map((p) => h('code', null, p))) : null,
      ),
    ),
  );
}

function revisionsView(rows: WeftRevisionRow[]): HTMLElement {
  if (!rows.length) return h('p.hx-faint', null, 'No revisions.');
  return h(
    'ol.hx-revs',
    null,
    rows.map((r) =>
      h(
        'li',
        null,
        h('code', null, r.id),
        h('span.hx-faint', null, ` ${r.kind}`),
        r.turn !== null ? h('span.hx-faint', null, ` · turn ${r.turn}`) : null,
        r.model ? h('span.hx-faint', null, ` · ${r.model}`) : null,
        h('span.hx-faint', null, ` · ${plural(r.tool_calls.length, 'tool call')} · ${plural(r.files, 'file')} · ${nameOfLabel(r.actor)}`),
      ),
    ),
  );
}

function recordsView(records: WeftRecord[]): HTMLElement {
  if (!records.length) return h('p.hx-faint', null, 'No records.');
  return h(
    'div.hx-records',
    null,
    [...records]
      .sort((a, b) => a.seq - b.seq)
      .map((r) =>
        h(
          'details',
          { class: 'hx-record' },
          h(
            'summary',
            null,
            h('span.hx-rec-type', null, r.type),
            h('span.hx-faint', null, ` ${r.revision} · ${nameOfLabel(r.actor)}`),
            ` · ${ago(r.at_ms)}`,
            r.via_console ? badge('from the console', 'accent') : null,
          ),
          h('p.hx-faint', null, `key `, h('code', { title: r.key }, shortKey(r.key)), ` · seq ${r.seq}`),
          h('pre.hx-json', null, JSON.stringify(r.predicate, null, 2)),
        ),
      ),
  );
}

const OP_CLASS: Record<string, string> = { '+': 'add', '~': 'chg', '-': 'del', '>': 'mv' };

function treeView(tree: WeftChangeTree | null): HTMLElement {
  if (!tree) return h('p.hx-faint', null, 'No tree for this change yet.');
  const langs = Object.entries(tree.languages)
    .map(([k, v]) => `${k} ${v}`)
    .join(', ');
  return h(
    'div.hx-tree-view',
    null,
    h('p.hx-faint', null, `+${tree.add} −${tree.del} · ${plural(tree.files.length, 'file')}${langs ? ` · ${langs}` : ''}`),
    tree.files.map((f) =>
      h(
        'details',
        { class: 'hx-tfile', open: true },
        h('summary', null, h('span.hx-op', { class: `op-${OP_CLASS[f.op] ?? 'chg'}` }, f.op), h('code', null, f.path), h('span.hx-faint', null, ` +${f.add} −${f.del}`)),
        h(
          'ul.hx-syms',
          null,
          f.symbols.map((s) => symbolItem(s)),
        ),
      ),
    ),
  );
}

function symbolItem(s: WeftSymbol): HTMLElement {
  const prov = [...new Set(s.provenance.map((p) => `${nameOfActor(p.by)}${p.model ? ` · ${p.model}` : ''}${p.turn !== null ? ` · turn ${p.turn}` : ''}`))];
  const touches = s.touches.map((t) => `${t.how} ${t.to}`);
  return h(
    'li',
    null,
    h('span.hx-op', { class: `op-${OP_CLASS[s.op] ?? 'chg'}`, title: s.op }, s.op),
    h('code', null, s.qualified),
    h('span.hx-faint', null, ` ${s.kind} · lines ${s.start_line}–${s.end_line}`),
    prov.length ? h('span.hx-prov', null, `by ${prov.join('; ')}`) : null,
    touches.length ? h('span.hx-touch', null, touches.join('; ')) : null,
  );
}

function diffView(diff: WeftDiff | null): HTMLElement {
  if (!diff) return h('p.hx-faint', null, 'No diff for this change yet.');
  if (!diff.files.length) return h('p.hx-faint', null, 'No changes.');
  return h(
    'div.hx-diffs',
    null,
    diff.files.map((f) =>
      h(
        'details',
        { class: 'hx-dfile' },
        h('summary', null, h('code', null, f.path), h('span.hx-faint', null, ` +${f.add} −${f.del}`)),
        h(
          'pre.hx-diff',
          null,
          f.hunks.flatMap((hk) => [
            h('div.hx-hunk', null, `@@ -${hk.old_start},${hk.old_lines} +${hk.new_start},${hk.new_lines} @@`),
            ...hk.lines.map(([op, text]) => h('div.hx-dl', { class: diffClass(op) }, h('span.hx-dop', null, op), text)),
          ]),
        ),
      ),
    ),
  );
}
