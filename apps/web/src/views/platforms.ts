// Git platforms: the owner authorizes the plaza on GitHub, GitLab and Forgejo, chooses what it may use there, and
// connects a granted repository to a project. Three add cards (each with a "Use a token" disclosure), the
// authorizations grouped by host with their grants panel open under one of them, and a connect dialog. The list is
// read on load and on each `platform` event; a flow the owner starts is polled until it ends.

import '../styles/proto/settings-b.css';
import { weft } from '../api-weft';
import {
  platforms,
  type ConnectResult,
  type FlowStart,
  type FlowStatus,
  type PlatformIntegration,
  type PlatformKind,
  type PlatformLevel,
  type PlatformList,
  type PlatformNamespace,
  type PlatformProject,
  type PlatformRepo,
  type PlatformConnection,
} from '../api-platforms';
import { on } from '../store';
import { attempt, badge, card, field, input, select, toast } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
import { btn, dialog, emptyBox, pageHead } from './mcp-common';
import {
  GITLAB_SCOPES,
  KIND_LABEL,
  KIND_ORDER,
  accountText,
  connectResultText,
  connectedCount,
  connectionText,
  flowOutcome,
  grantState,
  grantsBody,
  groupByHost,
  hasGrants,
  hostName,
  identityLine,
  levelLabel,
  namespaceLabel,
  registrationFor,
  registrationProblem,
  repositorySelectionLabel,
  setAll,
  setRepo,
  stateLine,
  targetPlaceholder,
  tokenBody,
  tokenProblem,
  viaLabel,
  type GrantState,
  type HostGroup,
} from './platforms-model';
import type { Cleanup } from './index';

/** How often a started flow is asked how it went. */
const FLOW_POLL_MS = 2_000;
/** A burst of `platform` events reloads once. */
const RELOAD_DEBOUNCE_MS = 250;
/** Where the kinds are, when the hub's list has no default host for one. */
const FALLBACK_HOST: Record<PlatformKind, string> = { github: 'https://github.com', gitlab: 'https://gitlab.com', forgejo: 'https://codeberg.org' };
const LEVELS: PlatformLevel[] = ['user', 'org', 'repo'];

/** A flow the owner started and the screen waits on: the page is open in another tab. */
interface Waiting { kind: PlatformKind; state: string; go: string; label: string; blocked: boolean }

/** The grants panel open under one authorization. */
interface Panel {
  id: string;
  /** The namespaces it reaches; null while they load. */
  namespaces: PlatformNamespace[] | null;
  error: string | null;
  /** The checkboxes, as the owner has them (the start is the authorization's grants). */
  draft: GrantState;
  /** The namespaces whose repositories are listed: each one's repositories (null while they load), or its error. */
  expanded: Map<string, { repos: PlatformRepo[] | null; error: string | null }>;
  saving: boolean;
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** The nearest ancestor that scrolls (the console's main area), or the document's scroller. */
function scrollParent(el: HTMLElement): HTMLElement | null {
  for (let p = el.parentElement; p; p = p.parentElement) {
    const o = getComputedStyle(p).overflowY;
    if ((o === 'auto' || o === 'scroll') && p.scrollHeight > p.clientHeight) return p;
  }
  return (document.scrollingElement as HTMLElement | null) ?? null;
}

/** A sheet whose buttons close it: `foot` gets the function that closes it. Returns that function. */
function sheet(title: string, body: Node[], foot: (done: () => void) => Node[]): () => void {
  let closer: () => void = () => undefined;
  const done = (): void => closer();
  closer = dialog(title, body, foot(done));
  return done;
}

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  let alive = true;
  let data: PlatformList | undefined;
  /** Connected repositories across the Weft repos; null when Weft could not be read. */
  let connected: number | null = null;
  let failure: string | undefined;
  let loads = 0;
  let reloadTimer: ReturnType<typeof setTimeout> | undefined;
  let pollTimer: ReturnType<typeof setTimeout> | undefined;
  let waiting: Waiting | null = null;
  let panel: Panel | null = null;
  /** The element the open grants panel is drawn into; a panel change redraws only this. */
  let panelEl: HTMLElement | null = null;
  /** Each add card's block (the part that depends on its host), redrawn as the host is typed. */
  const blocks = new Map<PlatformKind, HTMLElement>();
  let connectClose: (() => void) | undefined;
  /** Non-secret inputs as typed, so a redraw keeps them. Secrets are never kept. */
  const drafts: Record<string, string> = {};
  /** Whether the owner opened "Add a platform" (it starts closed, as the prototype has it). */
  let addOpen: boolean | null = null;

  const defaultHost = (kind: PlatformKind): string => data?.kinds.find((k) => k.kind === kind)?.default_host ?? FALLBACK_HOST[kind];
  /** The host a card acts on: what is typed, or the kind's default when nothing is. */
  const hostOf = (kind: PlatformKind): string => (drafts[`host:${kind}`] ?? '').trim() || defaultHost(kind);

  // ---- Drawing -----------------------------------------------------------------

  function draw(): void {
    if (!alive) return;
    panelEl = null;
    blocks.clear();
    const head = pageHead('Git platforms');
    if (failure !== undefined) {
      replace(root, head, emptyBox('triangle-alert', 'The platforms could not load', failure, btn('Try again', () => void load(), { kind: 'primary' })));
      return;
    }
    if (!data) {
      replace(root, head, h('p.plat-muted', null, 'Loading…'));
      return;
    }
    const d = data;
    if (panel && !d.integrations.some((i) => i.id === panel?.id)) panel = null;
    const needsYou = d.integrations.filter((i) => !!i.error).length;
    // A redraw keeps the reader where they were.
    const scroller = scrollParent(root);
    const top = scroller?.scrollTop ?? 0;
    // The head, the top line, what is authorized, then the way to add more (folded away once something is).
    replace(
      root,
      head,
      h('p.plat-line', null, stateLine({ authorizations: d.integrations.length, connected, needsYou })),
      d.integrations.length > 0 ? authSection(d) : null,
      addSection(d.integrations.length === 0),
    );
    renderPanel();
    if (scroller) scroller.scrollTop = top;
  }

  async function load(): Promise<void> {
    const seq = ++loads;
    try {
      const [list, repos] = await Promise.all([platforms.list(), weft.repos().catch(() => null)]);
      if (!alive || seq !== loads) return;
      data = list;
      connected = repos ? connectedCount(repos) : null;
      failure = undefined;
    } catch (err) {
      if (!alive || seq !== loads) return;
      failure = message(err);
    }
    draw();
    // The expanded namespaces' repositories may have new connections or grants.
    if (panel) for (const ns of [...panel.expanded.keys()]) void loadRepos(panel, ns);
  }

  function scheduleLoad(): void {
    if (reloadTimer !== undefined) clearTimeout(reloadTimer);
    reloadTimer = setTimeout(() => {
      reloadTimer = undefined;
      void load();
    }, RELOAD_DEBOUNCE_MS);
  }

  // ---- Add cards -----------------------------------------------------------------

  function addSection(first: boolean): Node | Node[] {
    const cards = h('div.plat-grid', null, KIND_ORDER.map((kind) => addCard(kind)));
    if (first) return [h('h2.plat-sec', null, 'Add'), cards];
    const fold = h('details.plat-add', { open: addOpen === true }, h('summary', null, 'Add a platform'), cards) as HTMLDetailsElement;
    fold.addEventListener('toggle', () => {
      addOpen = fold.open;
    });
    return fold;
  }

  function addCard(kind: PlatformKind): HTMLElement {
    const hostInput = input({ type: 'text', value: drafts[`host:${kind}`] ?? defaultHost(kind), autocomplete: 'off', spellcheck: 'false', placeholder: defaultHost(kind), 'aria-label': 'Host' });
    hostInput.addEventListener('input', () => {
      drafts[`host:${kind}`] = hostInput.value;
      renderBlock(kind);
    });
    const slot = h('div.plat-stack', null);
    blocks.set(kind, slot);
    renderBlock(kind);
    return card(KIND_LABEL[kind], h('div.plat-stack', null, field('Host', hostInput, 'Change it only for a self-hosted instance.'), slot, tokenBlock(kind)));
  }

  /** The part of a card that depends on its host: the GitHub App, or the OAuth app and sign-in. */
  function renderBlock(kind: PlatformKind): void {
    const slot = blocks.get(kind);
    if (!slot || !data) return;
    replace(slot, kind === 'github' ? githubBlock(data, hostOf(kind)) : oauthBlock(data, kind, hostOf(kind)));
  }

  function githubBlock(d: PlatformList, host: string): (HTMLElement | null)[] {
    const app = registrationFor(d.registrations, 'github', host);
    if (app?.app) {
      const { html_url: url, name, owner } = app.app;
      return [
        h('p.plat-muted', null, "The plaza's App: ", h('a', { href: url, target: '_blank', rel: 'noopener' }, name), ` · owned by ${owner}`),
        h('div.plat-row', null, btn('Install on an account or organisation', () => void installGitHub(app.id), { kind: 'primary', icon: 'plus' })),
        waitingLine('github'),
      ];
    }
    const orgInput = input({ type: 'text', value: drafts['org:github'] ?? '', autocomplete: 'off', spellcheck: 'false', placeholder: 'optional', 'aria-label': 'Organisation that owns it' });
    orgInput.addEventListener('input', () => {
      drafts['org:github'] = orgInput.value;
    });
    return [
      field('Organisation that owns it', orgInput, 'Leave empty to own the App yourself.'),
      h('div.plat-row', null, btn('Create the GitHub App', () => void createGitHubApp(host, orgInput.value), { kind: 'primary', icon: 'plus' })),
      waitingLine('github'),
    ];
  }

  function oauthBlock(d: PlatformList, kind: PlatformKind, host: string): (HTMLElement | null)[] {
    const label = KIND_LABEL[kind];
    const reg = registrationFor(d.registrations, kind, host);
    if (reg) {
      return [
        h('p.plat-muted', null, `The app on ${hostName(host)} is registered (client ID ${reg.client_id}).`),
        h('div.plat-row', null, btn('Sign in', () => void startFlow(kind, label, () => platforms.authorize(reg.id)), { kind: 'primary', icon: 'key' })),
        waitingLine(kind),
      ];
    }
    const clientInput = input({ type: 'text', value: drafts[`client_id:${kind}`] ?? '', autocomplete: 'off', spellcheck: 'false', 'aria-label': 'Client ID' });
    clientInput.addEventListener('input', () => {
      drafts[`client_id:${kind}`] = clientInput.value;
    });
    const secretInput = input({ type: 'password', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', 'aria-label': 'Client secret' });
    return [
      h('p.plat-muted', null, `Register an OAuth application on ${hostName(host)} with this redirect URI:`),
      h('div.plat-row', null, h('code.plat-code', null, d.callback), btn('Copy', () => void attempt(() => navigator.clipboard.writeText(d.callback), 'Copied'), { kind: 'ghost', sm: true })),
      kind === 'gitlab' ? h('p.plat-muted', null, `Scopes: ${GITLAB_SCOPES}.`) : null,
      field('Client ID', clientInput),
      field('Client secret', secretInput),
      h('div.plat-row', null, btn('Save the app', () => void registerApp(kind, host, clientInput, secretInput), { kind: 'primary' })),
    ];
  }

  /** "Use a token": a token, its level, and the target when the level is not the owner's account. */
  function tokenBlock(kind: PlatformKind): HTMLElement {
    const tokenInput = input({ type: 'password', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off', 'aria-label': 'Token' });
    const levelSelect = select(
      LEVELS.map((l) => ({ value: l, label: levelLabel(l, kind) })),
      drafts[`level:${kind}`] ?? 'user',
    );
    levelSelect.setAttribute('aria-label', 'Level');
    const first = levelSelect.value as PlatformLevel;
    const targetInput = input({ type: 'text', value: drafts[`target:${kind}`] ?? '', autocomplete: 'off', spellcheck: 'false', placeholder: targetPlaceholder(first, kind), 'aria-label': 'Target' });
    targetInput.addEventListener('input', () => {
      drafts[`target:${kind}`] = targetInput.value;
    });
    const targetField = field('Target', targetInput, 'The organisation, group or repository the token is for.');
    targetField.hidden = first === 'user';
    levelSelect.addEventListener('change', () => {
      const next = levelSelect.value as PlatformLevel;
      drafts[`level:${kind}`] = next;
      targetField.hidden = next === 'user';
      targetInput.placeholder = targetPlaceholder(next, kind);
    });
    return h(
      'details.plat-disc',
      null,
      h('summary', null, 'Use a token'),
      h(
        'div.plat-stack',
        null,
        h('p.plat-muted', null, 'A personal, group or project token. It is kept in the vault and never shown again.'),
        field('Token', tokenInput),
        field('Level', levelSelect),
        targetField,
        h('div.plat-row', null, btn('Authorize', () => void authorizeWithToken(kind, tokenInput, levelSelect, targetInput), { kind: 'primary' })),
      ),
    );
  }

  // ---- Actions on the add cards ---------------------------------------------------

  async function createGitHubApp(host: string, org: string): Promise<void> {
    const owner = org.trim();
    await startFlow('github', 'GitHub', () => platforms.createGitHubApp(owner ? { host, org: owner } : { host }));
  }

  async function installGitHub(registration: string): Promise<void> {
    await startFlow('github', 'GitHub', () => platforms.installGitHub(registration));
  }

  async function registerApp(kind: PlatformKind, host: string, clientInput: HTMLInputElement, secretInput: HTMLInputElement): Promise<void> {
    const clientId = clientInput.value.trim();
    const secret = secretInput.value.trim();
    const problem = registrationProblem(clientId, secret);
    if (problem) {
      toast(problem, 'error');
      return;
    }
    // The secret leaves the field as it is sent.
    secretInput.value = '';
    const saved = await attempt(() => platforms.register({ kind, host, client_id: clientId, client_secret: secret }), 'App saved. Sign in to authorize.');
    if (saved) await load();
  }

  async function authorizeWithToken(kind: PlatformKind, tokenInput: HTMLInputElement, levelSelect: HTMLSelectElement, targetInput: HTMLInputElement): Promise<void> {
    const token = tokenInput.value;
    const level = levelSelect.value as PlatformLevel;
    const target = targetInput.value;
    const problem = tokenProblem(token, level, target);
    if (problem) {
      toast(problem, 'error');
      return;
    }
    // The token leaves the field as it is sent.
    tokenInput.value = '';
    const made = await attempt(() => platforms.byToken(tokenBody({ kind, host: hostOf(kind), token, level, target })), `Authorized on ${KIND_LABEL[kind]}.`);
    if (made) await load();
  }

  // ---- Flows ------------------------------------------------------------------------

  /** Starts a flow, opens its page in a new tab, and waits for it to end. */
  async function startFlow(kind: PlatformKind, label: string, start: () => Promise<FlowStart>): Promise<void> {
    stopPolling();
    let flow: FlowStart;
    try {
      flow = await start();
    } catch (err) {
      toast(message(err), 'error');
      return;
    }
    if (!alive) return;
    const tab = window.open(flow.go, '_blank');
    waiting = { kind, state: flow.state, go: flow.go, label, blocked: tab === null };
    draw();
    schedulePoll();
  }

  function schedulePoll(): void {
    pollTimer = setTimeout(() => void pollFlow(), FLOW_POLL_MS);
  }

  function stopPolling(): void {
    if (pollTimer !== undefined) clearTimeout(pollTimer);
    pollTimer = undefined;
  }

  async function pollFlow(): Promise<void> {
    pollTimer = undefined;
    const w = waiting;
    if (!w || !alive) return;
    let status: FlowStatus;
    try {
      status = await platforms.flow(w.state);
    } catch (err) {
      if (waiting !== w || !alive) return;
      waiting = null;
      toast(message(err), 'error');
      draw();
      return;
    }
    if (!alive || waiting !== w) return;
    const outcome = flowOutcome(status, w.label);
    if (!outcome) {
      schedulePoll();
      return;
    }
    waiting = null;
    toast(outcome.text, outcome.error ? 'error' : 'info');
    await load();
  }

  function cancelFlow(): void {
    stopPolling();
    waiting = null;
    draw();
  }

  function waitingLine(kind: PlatformKind): HTMLElement | null {
    const w = waiting;
    if (!w || w.kind !== kind) return null;
    return h(
      'p.plat-wait',
      null,
      `Waiting for ${w.label}… `,
      h('a', { href: w.go, target: '_blank', rel: 'noopener' }, 'Open the page'),
      w.blocked ? h('span.plat-muted', null, ' The browser blocked the new tab.') : null,
      ' ',
      btn('Cancel', cancelFlow, { kind: 'ghost', sm: true }),
    );
  }

  // ---- Authorizations -----------------------------------------------------------------

  function authSection(d: PlatformList): Node[] {
    return [h('h2.plat-sec', null, 'Authorizations'), ...groupByHost(d.integrations).map((g) => hostGroup(g))];
  }

  function hostGroup(g: HostGroup): HTMLElement {
    return h(
      'div.plat-grp-wrap',
      null,
      h('h3.plat-grp', null, g.label, h('span.plat-grp-sub', null, hostName(g.host))),
      h('div.plat-grid', null, g.items.map((i) => authCard(i))),
    );
  }

  function authCard(i: PlatformIntegration): HTMLElement {
    const who = identityLine(i.identity, i.host);
    const selection = repositorySelectionLabel(i.repository_selection);
    const granted = hasGrants(i.grants);
    const panelSlot = panel?.id === i.id ? h('div.plat-ns-panel.plat-stack', null) : null;
    if (panelSlot) panelEl = panelSlot;
    return card(
      accountText(i),
      h(
        'div.plat-stack',
        null,
        h('div.plat-row', null, badge(KIND_LABEL[i.kind]), badge(levelLabel(i.level, i.kind)), badge(viaLabel(i.via))),
        h('p', { class: who.warn ? 'plat-warn' : 'plat-muted' }, who.text),
        selection ? h('p.plat-muted', null, selection, i.manage_url ? ' · ' : null, i.manage_url ? h('a', { href: i.manage_url, target: '_blank', rel: 'noopener' }, 'Change on GitHub') : null) : null,
        i.error ? h('p.plat-warn', null, lu('triangle-alert', 16), ` ${i.error}`) : null,
        h(
          'div.plat-row',
          null,
          btn('Choose what the plaza may use', () => openPanel(i), { kind: granted ? undefined : 'primary', sm: true }),
          btn('Sync', () => void sync(i), { icon: 'refresh-cw', sm: true }),
          btn('Remove', () => confirmRemove(i), { kind: 'danger', sm: true }),
        ),
        panelSlot,
      ),
    );
  }

  async function sync(i: PlatformIntegration): Promise<void> {
    const synced = await attempt(() => platforms.sync(i.id), 'Synced.');
    if (synced) await load();
  }

  function confirmRemove(i: PlatformIntegration): void {
    sheet(
      `Remove ${accountText(i)}?`,
      [h('p', null, 'The plaza forgets this authorization and its tokens. Nothing changes on the platform itself.')],
      (done) => [
        btn('Cancel', done, { kind: 'ghost' }),
        btn(
          'Remove',
          () => {
            done();
            void remove(i);
          },
          { kind: 'danger' },
        ),
      ],
    );
  }

  async function remove(i: PlatformIntegration): Promise<void> {
    const removed = await attempt(() => platforms.remove(i.id), 'Removed.');
    if (!removed) return;
    if (panel?.id === i.id) panel = null;
    await load();
  }

  // ---- The grants panel ---------------------------------------------------------------

  function openPanel(i: PlatformIntegration): void {
    if (panel?.id === i.id) return;
    panel = { id: i.id, namespaces: null, error: null, draft: grantState(i.grants), expanded: new Map(), saving: false };
    draw();
    void loadNamespaces(panel);
  }

  function closePanel(): void {
    panel = null;
    draw();
  }

  async function loadNamespaces(p: Panel): Promise<void> {
    try {
      const list = await platforms.namespaces(p.id);
      if (panel !== p) return;
      p.namespaces = list;
    } catch (err) {
      if (panel !== p) return;
      p.error = message(err);
    }
    renderPanel();
  }

  async function loadRepos(p: Panel, ns: string): Promise<void> {
    const before = p.expanded.get(ns)?.repos ?? null;
    p.expanded.set(ns, { repos: before, error: null });
    renderPanel();
    try {
      const repos = await platforms.repos(p.id, ns);
      if (panel !== p || !p.expanded.has(ns)) return;
      p.expanded.set(ns, { repos, error: null });
    } catch (err) {
      if (panel !== p || !p.expanded.has(ns)) return;
      p.expanded.set(ns, { repos: before, error: message(err) });
    }
    renderPanel();
  }

  function toggleNamespace(p: Panel, ns: string): void {
    if (p.expanded.has(ns)) {
      p.expanded.delete(ns);
      renderPanel();
      return;
    }
    void loadRepos(p, ns);
  }

  async function savePanel(p: Panel): Promise<void> {
    if (p.saving) return;
    p.saving = true;
    renderPanel();
    const updated = await attempt(() => platforms.saveGrants(p.id, grantsBody(p.draft)), 'Saved.');
    p.saving = false;
    if (updated) await load();
    else renderPanel();
  }

  /** Redraws the open panel only, so the checkboxes keep their place. */
  function renderPanel(): void {
    const p = panel;
    const el = panelEl;
    if (!p || !el) return;
    const i = data?.integrations.find((x) => x.id === p.id);
    if (!i) return;
    replace(el, panelBody(p, i));
  }

  function panelBody(p: Panel, i: PlatformIntegration): (Node | null)[] {
    const head = h('div.plat-row', null, h('h4', { style: 'margin:0;flex:1 1 auto' }, 'Choose what the plaza may use'), btn('Done', closePanel, { kind: 'ghost', sm: true }));
    if (!p.namespaces) return [head, p.error ? h('p.plat-err', { role: 'alert' }, p.error) : h('p.plat-muted', null, 'Loading…')];
    const namespaces = p.namespaces;
    return [
      head,
      p.error ? h('p.plat-err', { role: 'alert' }, p.error) : null,
      namespaces.length === 0 ? h('p.plat-muted', null, 'This authorization reaches no namespace.') : null,
      ...namespaces.map((ns) => namespaceRow(p, i, ns)),
      namespaces.length > 0 ? h('div.plat-row', null, btn('Save', () => void savePanel(p), { kind: 'primary', disabled: p.saving })) : null,
    ];
  }

  function namespaceRow(p: Panel, i: PlatformIntegration, ns: PlatformNamespace): HTMLElement {
    const draft = p.draft[ns.name];
    const all = input({ type: 'checkbox' });
    all.checked = draft?.all === true;
    all.addEventListener('change', () => {
      setAll(p.draft, ns.name, all.checked);
      renderPanel();
    });
    const open = p.expanded.has(ns.name);
    return h(
      'div.plat-ns',
      null,
      h(
        'div.plat-row',
        null,
        h('label.check', null, all, ' All repositories'),
        h('span.plat-repo-name', null, ns.name),
        badge(namespaceLabel(ns.kind)),
        btn(open ? 'Hide repositories' : 'Show repositories', () => toggleNamespace(p, ns.name), { kind: 'ghost', sm: true }),
      ),
      open ? repoList(p, i, ns.name) : null,
    );
  }

  function repoList(p: Panel, i: PlatformIntegration, ns: string): Node[] | HTMLElement {
    const entry = p.expanded.get(ns);
    if (!entry || entry.repos === null) return h('p.plat-muted', null, entry?.error ?? 'Loading repositories…');
    if (entry.repos.length === 0) return h('p.plat-muted', null, 'No repositories in this namespace.');
    const allOn = p.draft[ns]?.all === true;
    return entry.repos.map((r) => repoRow(p, i, ns, r, allOn));
  }

  function repoRow(p: Panel, i: PlatformIntegration, ns: string, r: PlatformRepo, allOn: boolean): HTMLElement {
    const drafted = p.draft[ns]?.repos.includes(r.name) === true;
    const box = input({ type: 'checkbox' });
    box.checked = allOn || drafted;
    box.disabled = allOn;
    box.addEventListener('change', () => {
      setRepo(p.draft, ns, r.name, box.checked);
      renderPanel();
    });
    return h(
      'div.plat-repo',
      null,
      h(
        'div.plat-row',
        null,
        h('label.check', null, box, h('span.plat-repo-name', null, r.name)),
        r.private ? badge('Private') : null,
        r.can_push ? null : h('span.plat-muted', null, "Can't push"),
      ),
      h('div', null, repoConnections(i, r, drafted || allOn)),
    );
  }

  /** A granted repository shows its connections, or offers to connect it. */
  function repoConnections(i: PlatformIntegration, r: PlatformRepo, chosen: boolean): Node[] | HTMLElement | null {
    if (!r.granted) return chosen ? h('span.plat-muted', null, 'Save to connect it to a project.') : null;
    if (r.connections.length > 0) {
      return r.connections.map((c) => h('div.plat-row', null, h('span', null, connectionText(c)), btn('Disconnect', () => void disconnect(i, c), { kind: 'ghost', sm: true })));
    }
    if (!r.can_push) return h('span.plat-muted', null, 'This authorization cannot push to it, so it cannot connect.');
    return btn('Connect to a project', () => openConnect(i, r), { sm: true });
  }

  async function disconnect(i: PlatformIntegration, c: PlatformConnection): Promise<void> {
    const done = await attempt(() => platforms.disconnect(i.id, { weft_repo: c.weft_repo, connection: c.connection }), `Disconnected from ${c.weft_repo}.`);
    if (done) await load();
  }

  // ---- Connect dialog -------------------------------------------------------------------

  /** Opens the connect dialog: the projects are read first, then the form (or the reason there is no form). */
  function openConnect(i: PlatformIntegration, r: PlatformRepo): void {
    void platforms.projects().then(
      (list) => {
        if (!alive) return;
        connectClose?.();
        connectClose = connectForm(i, r, list.filter((p) => !p.archived));
      },
      (err: unknown) => {
        if (!alive) return;
        connectClose?.();
        connectClose = sheet('Connect to a project', [h('p.plat-err', { role: 'alert' }, message(err))], (done) => [btn('Close', done, { kind: 'ghost' })]);
      },
    );
  }

  function connectForm(i: PlatformIntegration, r: PlatformRepo, projects: PlatformProject[]): () => void {
    if (projects.length === 0) {
      return sheet('Connect to a project', [h('p', null, 'There is no project to connect to yet.')], (done) => [btn('Close', done, { kind: 'ghost' })]);
    }
    const project = select(projects.map((p) => ({ value: p.id, label: p.title || p.id })));
    project.setAttribute('aria-label', 'Project');
    const signer = input({ type: 'checkbox' });
    signer.checked = true;
    const pusher = input({ type: 'checkbox' });
    const identity = identityLine(i.identity, i.host);
    const who = i.identity.login || i.account.login || hostName(i.host);
    const error = h('p.plat-err', { role: 'alert' });
    let busy = false;
    const connectBtn = btn('Connect', () => void submit(), { kind: 'primary' });
    const submit = async (): Promise<void> => {
      if (busy) return;
      busy = true;
      error.textContent = '';
      connectBtn.disabled = true;
      try {
        const res: ConnectResult = await platforms.connect(i.id, { repo: r.full_name, project: project.value, sign: signer.checked, export: pusher.checked });
        const said = connectResultText(res);
        close();
        connectClose = sheet('Connect to a project', [h('p', { class: said.warn ? 'plat-warn' : null }, said.text)], (close) => [btn('Close', close, { kind: 'primary' })]);
        await load();
      } catch (err) {
        error.textContent = message(err);
        busy = false;
        connectBtn.disabled = false;
      }
    };
    const close = sheet(
      'Connect to a project',
      [
        h('p', null, `Connect ${r.full_name}. Work that lands in the project exports to it.`),
        field('Project', project),
        h('label.check', null, signer, ` Register the hub's signing key on ${who} (exports show as Verified)`),
        h('label.check', null, pusher, ' Push now'),
        h('p', { class: identity.warn ? 'plat-warn' : 'plat-muted' }, identity.text),
        error,
      ],
      (closeFn) => [btn('Cancel', closeFn, { kind: 'ghost' }), connectBtn],
    );
    return close;
  }

  // ---- Start -------------------------------------------------------------------------

  const off = on('platform', scheduleLoad);
  draw();
  void load();

  return () => {
    alive = false;
    off();
    if (reloadTimer !== undefined) clearTimeout(reloadTimer);
    stopPolling();
    connectClose?.();
    connectClose = undefined;
  };
}
