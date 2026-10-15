// The Metrics view (`#/metrics?…`): the page shell (the header with the breadcrumb and controls, the time tier line,
// the scope tree, the grouped tabs and the panel grid) and the flow around it: the URL state, the hub requests, live
// frames, pins, overlays and the keyboard. The numbers all come from the hub; this file lays them out.

import '../styles/metrics.css';
import type { Cleanup } from './index';
import { api } from '../api';
import { on } from '../store';
import { button, select, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import { localOffset } from '../metrics/format';
import { HoverBus } from '../metrics/hover';
import { closeOverlay, dialog, drawer, help, menu, overlayOpen, palette, type PaletteItem } from '../metrics/overlays';
import { renderPanel, type PanelCtx, type PanelHandle } from '../metrics/panels';
import { ruleDialog, settingsDialog } from '../metrics/dialogs';
import { RANGES, type LiveFrame, type LiveLease, type MetricDef, type Panel, type Rule, type Saved, type ScopeNode, type View } from '../metrics/model';
import { LeaseKeeper, liveApplies, loadPrefs, parseHash, queryOf, savePrefs, toHash, togglePin, togglePinnedScope, visit, type MetricsState } from '../metrics/state';

/** The group-by choices. */
const GROUPS = ['provider', 'model', 'role', 'run', 'agent', 'engine'];
/** The ranges that take live frames. */
const LIVE_RANGES = ['15m', '1h'];
/** How long the scope tree stays fresh before it is fetched again. */
const SCOPE_TTL_MS = 60_000;
/** The freshness stamps and the live frame gate both move once a second. */
const TICK_MS = 1_000;
/** A resize redraws the panels once it has settled. */
const RESIZE_DEBOUNCE_MS = 120;
/** How long a rule stays muted from a panel's alert row. */
const MUTE_MS = 3_600_000;
/** The grid width used before the grid has been laid out. */
const FALLBACK_WIDTH = 960;

/** Where a click or a shortcut sends the view. */
interface Target {
  scope?: string;
  tab?: string;
  trace?: string;
}

/** A readable message for a failed request. */
function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** A local date and time, for a view that looks back. */
function clock(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}

/** A fraction of the range as a whole percentage. */
function percent(fraction: number): string {
  return (fraction * 100).toFixed(0);
}

function isNode(n: ScopeNode | undefined): n is ScopeNode {
  return n !== undefined;
}

/** The scopes grouped by their group, in the order the hub gave them. */
function groupScopes(nodes: ScopeNode[]): [string, ScopeNode[]][] {
  const byGroup = new Map<string, ScopeNode[]>();
  for (const n of nodes) {
    const list = byGroup.get(n.group) ?? [];
    list.push(n);
    byGroup.set(n.group, list);
  }
  return Array.from(byGroup);
}

/** The catalog entry as a description list (for "About this metric"). */
function aboutList(def: MetricDef): HTMLElement {
  const rows: [string, string][] = [
    ['What it is', def.description],
    ['Unit', def.unit],
    ['Source', def.source],
    ['Collected at', def.level],
    ['Cost', def.cost],
  ];
  const nodes: HTMLElement[] = [];
  for (const [term, value] of rows) nodes.push(h('dt', {}, term), h('dd', {}, value));
  return h('dl.mx-kv', {}, nodes);
}

/** The compare select's value for a compare state (a scope comparison shows as "another scope"). */
function compareKey(compare: string): string {
  return compare.startsWith('scope:') ? 'scope' : compare;
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  void params;

  // ---- State ----
  // The route (tab, scope, range, …) comes from the hash; live, annotations and density come from the preferences.
  let state: MetricsState = { ...parseHash(location.hash), ...loadPrefs() };
  let view: View | null = null;
  let scopes: ScopeNode[] = [];
  let scopesAt = 0;
  let saved: Saved = { scopes: [], pins: [], recent: [], starter: false };
  let catalog: MetricDef[] = [];
  let loadSeq = 0;
  let disposed = false;
  /** The time the panels' data is as of: the view's end, or the last live frame. */
  let freshMs = Date.now();
  const handles = new Map<string, PanelHandle>();
  const hover = new HoverBus();
  const subscriptions: (() => void)[] = [];
  const lease = new LeaseKeeper((l: LiveLease) => api.metricsLive(l), activeLease);
  // Live frames: the newest one is kept, and drawn at most once a second.
  let pendingFrame: LiveFrame | null = null;
  let frameTimer: ReturnType<typeof setTimeout> | undefined;
  let frameRaf = 0;
  let lastFrameAt = 0;
  let freshTimer: ReturnType<typeof setInterval> | undefined;
  let resizeTimer: ReturnType<typeof setTimeout> | undefined;

  // ---- Page shell ----
  const starBtn = h('button.mx-star', { type: 'button', title: 'Pin this scope (f)', onclick: () => toggleScopePin() }, '☆');
  const crumbsEl = h('div.mx-crumbs');
  const scopeBtn = button('Scope ▾', () => treeEl.classList.toggle('mx-open'), { kind: 'ghost', size: 'sm' });
  scopeBtn.classList.add('mx-scope-btn');
  const findBtn = button('Find… /', () => openFind(), { kind: 'ghost', size: 'sm', title: 'Find (/ or Ctrl K)' });
  const shiftBack = h('button', { type: 'button', title: 'Earlier ([)', onclick: () => shiftBy(-1) }, '‹');
  const shiftFwd = h('button', { type: 'button', title: 'Later (])', onclick: () => shiftBy(1) }, '›');
  const shiftSeg = h('div.mx-seg', {}, shiftBack, shiftFwd);
  const rangeSeg = h('div.mx-seg');
  const liveBtn = button('Live', () => toggleLive(), { kind: 'ghost', size: 'sm', title: 'Live (l)' });
  const groupSel = select(GROUPS.map((g) => ({ value: g, label: 'by ' + g })), state.group);
  groupSel.setAttribute('aria-label', 'Group by');
  groupSel.addEventListener('change', () => navigate({ ...state, group: groupSel.value }));
  const compareSel = select(
    [
      { value: 'prev', label: 'vs previous period' },
      { value: 'week', label: 'vs same time last week' },
      { value: 'scope', label: 'vs another scope…' },
      { value: 'none', label: 'no comparison' },
    ],
    compareKey(state.compare),
  );
  compareSel.setAttribute('aria-label', 'Compare');
  compareSel.addEventListener('change', onCompareChange);
  const annotBtn = button('Annotations', () => toggleAnnotations(), { kind: 'ghost', size: 'sm', title: 'Annotations (a)' });
  const settingsBtn = button('Settings', () => void openSettings(), { kind: 'ghost', size: 'sm' });
  const exportLink = h('a.btn.ghost.sm', { href: '#', download: '' }, 'Export');
  const head = h(
    'header.mx-head',
    {},
    h('div.mx-title', {}, h('h1', {}, 'Metrics ', starBtn), crumbsEl),
    h('div.mx-ctl', {}, scopeBtn, findBtn, shiftSeg, rangeSeg, liveBtn, groupSel, compareSel, annotBtn, exportLink, settingsBtn),
  );
  const tierEl = h('div.mx-tier');
  const treeFilter = h('input', { type: 'search', placeholder: 'Find a node, VM, agent, model…', 'aria-label': 'Filter scopes', oninput: () => renderTreeList() });
  const treeList = h('div.mx-tree-list');
  const treeEl = h('nav.mx-tree', { 'aria-label': 'Scopes' }, treeFilter, treeList);
  const tabsEl = h('div.mx-tabs', { role: 'tablist' });
  const gridEl = h('div.mx-grid');
  const bodyEl = h('div.mx-body', {}, treeEl, h('section', {}, tabsEl, gridEl));
  const resizeObs = new ResizeObserver(() => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(redrawAll, RESIZE_DEBOUNCE_MS);
  });

  // ---- URL state and requests ----

  /** Moves to a new state: the hash, the chrome, the tree, the live lease, then the view itself. */
  function navigate(next: MetricsState): void {
    state = next;
    writeHash();
    renderControls();
    renderTreeList();
    syncLive();
    void load();
  }

  /** Moves to a scope, tab or trace. A new scope clears the zoom and the trace and is remembered as recent. */
  function go(target: Target): void {
    const scope = target.scope ?? state.scope;
    const tab = target.tab ?? state.tab;
    const scopeChanged = scope !== state.scope;
    if (scopeChanged) setSaved(visit(saved, scope));
    const trace = target.trace ?? (scopeChanged || tab !== state.tab ? null : state.trace);
    navigate({ ...state, scope, tab, trace, zoom: scopeChanged ? null : state.zoom });
  }

  /** Writes the state into the hash, only when it differs (the console's router reads the hash). */
  function writeHash(): void {
    const hash = toHash(state);
    if (location.hash === hash) return;
    try {
      history.replaceState(null, '', hash);
    } catch {
      // A sandboxed page cannot rewrite its hash; the view still works.
    }
  }

  /** Fetches the view for the state, and applies it unless a newer request has started. */
  async function load(): Promise<void> {
    const seq = ++loadSeq;
    if (Date.now() - scopesAt > SCOPE_TTL_MS) void loadScopes();
    if (!view) setGridMessage('Loading…');
    try {
      const next = await api.metricsView(queryOf(state, gridWidth(), localOffset()));
      if (!disposed && seq === loadSeq) applyView(next);
    } catch (err) {
      if (!disposed && seq === loadSeq) setGridMessage(`Could not load metrics: ${errorText(err)}`);
    }
  }

  /** Shows a view the hub composed. The hub may resolve the tab (a scope may not have the one asked for). */
  function applyView(next: View): void {
    view = next;
    freshMs = next.to_ms;
    if (next.tab !== state.tab) {
      state = { ...state, tab: next.tab };
      writeHash();
    }
    renderControls();
    renderTreeList();
    drawGrid();
    syncLive();
  }

  async function loadScopes(): Promise<void> {
    scopesAt = Date.now();
    try {
      scopes = await api.metricsScopes();
      if (!disposed) renderTreeList();
    } catch {
      scopesAt = 0;
    }
  }

  async function loadSaved(): Promise<void> {
    try {
      saved = await api.metricsSaved();
      if (!disposed) {
        renderStar();
        renderTreeList();
      }
    } catch {
      // Without the pins and recent scopes the tree still works.
    }
  }

  async function loadCatalog(): Promise<void> {
    try {
      catalog = await api.metricsCatalog();
    } catch {
      // "About this metric" then says there is no entry.
    }
  }

  /** Stores the pins and recent scopes; the local copy is what the page shows. */
  function setSaved(next: Saved): void {
    saved = next;
    void api.metricsSave(next).catch(() => toast('Could not save your pins', 'error'));
  }

  /** The hashchange listener: a hash the page did not write (a link, the back button) becomes the state. */
  function onHashChange(): void {
    if (!location.hash.startsWith('#/metrics')) return;
    const next = parseHash(location.hash);
    if (toHash(next) === toHash(state)) return;
    state = { ...next, live: state.live, annotations: state.annotations, compact: state.compact };
    renderControls();
    renderTreeList();
    syncLive();
    void load();
  }

  // ---- Header and tier line ----

  /** Brings the header, tabs and tier line up to the state and the view. */
  function renderControls(): void {
    renderCrumbs();
    renderStar();
    renderRanges();
    shiftFwd.disabled = state.shift >= 0;
    liveBtn.setAttribute('aria-pressed', String(state.live));
    annotBtn.setAttribute('aria-pressed', String(state.annotations));
    groupSel.value = state.group;
    compareSel.value = compareKey(state.compare);
    exportLink.href = api.metricsExportUrl(queryOf(state, gridWidth(), localOffset()));
    renderTabs();
    renderTier();
  }

  function renderCrumbs(): void {
    const v = view;
    if (!v) return;
    const path = v.scope.path;
    const last = path.length - 1;
    const nodes: Node[] = [];
    path.forEach(([label, id], i) => {
      if (i === last) {
        nodes.push(h('span.mx-here', {}, label.trim()));
        if (v.scope.siblings.length > 1) nodes.push(siblingButton(v.scope.siblings));
      } else {
        nodes.push(h('button', { type: 'button', onclick: () => go({ scope: id }) }, label.trim()), h('span', {}, '›'));
      }
    });
    replace(crumbsEl, nodes);
  }

  /** The ▾ beside the current scope: its siblings in a menu. */
  function siblingButton(siblings: [string, string][]): HTMLButtonElement {
    const btn = h('button.mx-sib', { type: 'button', title: 'Switch to a sibling', 'aria-label': 'Switch to a sibling' }, '▾');
    btn.addEventListener('click', (ev) => {
      ev.stopPropagation();
      menu(
        btn,
        siblings.map(([label, id]) => ({ label: (id === state.scope ? '● ' : '') + label.trim(), run: () => go({ scope: id }) })),
      );
    });
    return btn;
  }

  function renderStar(): void {
    const pinned = saved.scopes.includes(state.scope);
    starBtn.textContent = pinned ? '★' : '☆';
    starBtn.setAttribute('aria-pressed', String(pinned));
  }

  function renderRanges(): void {
    replace(
      rangeSeg,
      RANGES.map((r) =>
        h('button', { type: 'button', 'aria-pressed': String(r.key === state.range), onclick: () => navigate({ ...state, range: r.key, zoom: null }) }, r.key),
      ),
    );
  }

  /** The tier line: the window's end, the tier the hub read, the live text, the zoom and the comparison chips. */
  function renderTier(): void {
    const v = view;
    const end = v && state.shift ? `ending ${clock(v.to_ms)}` : 'ending now';
    const reading = v
      ? h(
          'span',
          {},
          `${end} · reading `,
          h('b', {}, v.tier),
          `, ${v.points} points per series · ${liveText()} · level: ${v.level}`,
          v.stepped_down ? ' · stepped down' : null,
        )
      : h('span', {}, end);
    const chips: Node[] = [];
    if (state.zoom) {
      chips.push(
        h('span.mx-chip', {}, `zoomed ${percent(state.zoom[0])}–${percent(state.zoom[1])}% of the range `, button('reset (z)', () => resetZoom(), { kind: 'ghost', size: 'sm' })),
      );
    }
    if (v && state.compare.startsWith('scope:')) {
      chips.push(h('span.mx-chip', {}, `comparing with ${v.compare_label} `, button('stop', () => stopCompare(), { kind: 'ghost', size: 'sm' })));
    }
    replace(tierEl, reading, chips);
  }

  function liveText(): string {
    if (!state.live) return 'live off';
    if (state.shift) return 'live pauses while looking back';
    if (!LIVE_RANGES.includes(state.range)) return 'live applies to the 15m and 1h ranges';
    if (document.hidden) return 'live paused while this page is hidden';
    return 'live: the scope on screen, drawn at most once a second';
  }

  /** The tabs, grouped; the Alerts tab carries the count of firing alerts. */
  function renderTabs(): void {
    const v = view;
    if (!v) {
      replace(tabsEl);
      return;
    }
    const nodes: Node[] = [];
    let group: string | null = null;
    for (const t of v.tabs) {
      if (t.group !== group) {
        group = t.group;
        nodes.push(h('span.mx-tg', { 'aria-hidden': 'true' }, t.group || '·'));
      }
      const count = t.key === 'alerts' ? v.firing : 0;
      const badge = count > 0 ? h('span.mx-badge', {}, String(count)) : null;
      nodes.push(h('button', { type: 'button', role: 'tab', 'aria-selected': String(t.key === state.tab), onclick: () => selectTab(t.key) }, t.label, badge));
    }
    replace(tabsEl, nodes);
    const current = tabsEl.querySelector('[aria-selected=true]');
    current?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }

  function visibleTabs(): { key: string; label: string }[] {
    return view ? view.tabs : [];
  }

  function selectTab(key: string): void {
    if (!visibleTabs().some((t) => t.key === key)) return;
    go({ tab: key });
    window.scrollTo({ top: 0, behavior: 'smooth' });
  }

  // ---- Scope tree ----

  /** The tree: the pinned scopes, the recent ones, then every scope by group; the filter narrows all of them. */
  function renderTreeList(): void {
    const q = treeFilter.value.trim().toLowerCase();
    const byId = new Map(scopes.map((n) => [n.id, n] as const));
    const groups: [string, ScopeNode[]][] = [];
    const pinned = saved.scopes.map((id) => byId.get(id)).filter(isNode);
    if (pinned.length) groups.push(['★ Pinned scopes', pinned]);
    const recent = saved.recent
      .filter((id) => !saved.scopes.includes(id) && id !== state.scope)
      .map((id) => byId.get(id))
      .filter(isNode);
    if (recent.length) groups.push(['Recent', recent]);
    groups.push(...groupScopes(scopes));
    const nodes: HTMLElement[] = [];
    for (const [label, list] of groups) {
      const rows = list.filter((n) => !q || n.label.toLowerCase().includes(q));
      if (!rows.length) continue;
      nodes.push(h('div.mx-grp', {}, label));
      for (const n of rows) nodes.push(treeNode(n));
    }
    replace(treeList, nodes.length ? nodes : h('div.mx-empty', {}, 'No scope matches.'));
  }

  function treeNode(n: ScopeNode): HTMLElement {
    const node = h(
      'button',
      {
        type: 'button',
        class: 'mx-node',
        'aria-current': String(n.id === state.scope),
        style: `padding-left: ${12 + n.depth * 14}px`,
        onclick: () => {
          treeEl.classList.remove('mx-open');
          go({ scope: n.id });
        },
      },
      n.status ? h('span.mx-st', { class: 'mx-' + n.status }) : null,
      n.label.trim(),
      n.tag ? h('span.mx-k', {}, n.tag) : null,
    );
    return node;
  }

  // ---- Panels ----

  function gridWidth(): number {
    return gridEl.clientWidth || FALLBACK_WIDTH;
  }

  function setGridMessage(text: string): void {
    disposeHandles();
    replace(gridEl, h('div.mx-empty', {}, text));
  }

  /** Draws every panel of the view. Live frames and resizes update the existing handles instead. */
  function drawGrid(): void {
    disposeHandles();
    const v = view;
    if (!v) return;
    const ctx = panelContext();
    const cards: HTMLElement[] = v.panels.map((p) => {
      const handle = renderPanel(p, ctx);
      handles.set(p.id, handle);
      return handle.el;
    });
    replace(gridEl, cards.length ? cards : h('div.mx-empty', {}, 'Nothing to show here yet.'));
    refreshFresh();
  }

  function disposeHandles(): void {
    for (const handle of handles.values()) handle.destroy();
    handles.clear();
  }

  function redrawAll(): void {
    for (const handle of handles.values()) handle.redraw();
  }

  /** What the panels can ask the view to do. */
  function panelContext(): PanelCtx {
    return {
      hover,
      annotations: view ? view.annotations : [],
      showAnnotations: state.annotations,
      compact: state.compact,
      tz: localOffset(),
      onZoom: zoomTo,
      onPoint: (_index, t) => {
        if (t !== null) void openAround(t);
      },
      go: (target) => go(target),
      filter: (f) => navigate({ ...state, filter: f || null }),
      mute: (rule) => runAction(api.metricsMute(rule, Date.now() + MUTE_MS), 'Muted for 1 hour'),
      ack: (rule, subject) => runAction(api.metricsAck(rule, subject), 'Acknowledged'),
      toggleRule: (id, enabled) => void setRuleOn(id, enabled),
      newRule: () => newRule(),
      older: () => toast('Older calls: open the call log'),
      pin: (p) => togglePanelPin(p),
      isPinned: (p) => isPinned(p),
      about: (p) => aboutPanel(p),
      copyLink: () => void copyLink(),
      liveNow: liveOn(),
      compareLabel: view ? view.compare_label : undefined,
    };
  }

  /** Runs an alert action, then reloads the view it may have changed. */
  function runAction(pending: Promise<unknown>, done: string): void {
    void pending.then(
      () => {
        if (disposed) return;
        toast(done);
        void load();
      },
      (err: unknown) => toast(errorText(err), 'error'),
    );
  }

  /** Turns an alert rule on or off. The hub treats an existing rule id sent with only `on` as a switch. */
  async function setRuleOn(id: string, enabled: boolean): Promise<void> {
    try {
      await api.metricsPutRule({ id, on: enabled } as unknown as Rule);
      if (!disposed) void load();
    } catch (err) {
      toast(errorText(err), 'error');
    }
  }

  function newRule(): void {
    ruleDialog(catalog, scopes, state.scope, saveRule);
  }

  async function saveRule(rule: Rule): Promise<void> {
    await api.metricsPutRule(rule);
    if (!disposed) void load();
  }

  function isPinned(p: Panel): boolean {
    return saved.pins.some((pin) => pin.tab === state.tab && pin.scope === state.scope && pin.panel === p.id);
  }

  function togglePanelPin(p: Panel): void {
    const wasPinned = isPinned(p);
    setSaved(togglePin(saved, { tab: state.tab, scope: state.scope, panel: p.id }));
    toast(wasPinned ? 'Unpinned' : 'Pinned to My dashboard');
  }

  function aboutPanel(p: Panel): void {
    const def = catalog.find((m) => m.id === p.metric);
    const body = def ? aboutList(def) : h('p', {}, 'No catalog entry for this panel.');
    dialog(p.title, body, [{ label: 'Close', primary: true }]);
  }

  async function copyLink(): Promise<void> {
    const url = `${location.origin}${location.pathname}${toHash(state)}`;
    try {
      await navigator.clipboard.writeText(url);
      toast('Link copied');
    } catch {
      toast(url);
    }
  }

  // ---- Charts: zoom, points and the "what changed" drawer ----

  /** A drag across a chart: the fractions are of the chart's window, which is the current zoom (or the whole range). */
  function zoomTo(fa: number, fb: number): void {
    const [z0, z1] = state.zoom ?? [0, 1];
    const a = z0 + Math.min(fa, fb) * (z1 - z0);
    const b = z0 + Math.max(fa, fb) * (z1 - z0);
    if (b <= a) return;
    navigate({ ...state, zoom: [a, b] });
  }

  function resetZoom(): void {
    if (state.zoom) navigate({ ...state, zoom: null });
  }

  /** A click on a point: what changed around its time, in the drawer. */
  async function openAround(t: number): Promise<void> {
    try {
      const around = await api.metricsAround(queryOf(state, gridWidth(), localOffset()), t);
      if (disposed) return;
      drawer(around, {
        go: (target) => go(target),
        zoom: (z) => navigate({ ...state, zoom: z }),
        traces: () => go({ tab: 'traces' }),
      });
    } catch (err) {
      toast(errorText(err), 'error');
    }
  }

  // ---- Live ----

  function liveOn(): boolean {
    return liveApplies(state) && !document.hidden;
  }

  /** The stream the live lease asks the hub for. */
  function activeLease(): LiveLease | null {
    return liveOn() ? { scope: state.scope, tab: state.tab, group: state.group, range: state.range } : null;
  }

  /** Called after anything that changes whether the view is live: the lease, the tickers, the tier line. */
  function syncLive(): void {
    lease.poke();
    if (liveOn()) startTicks();
    else stopTicks();
    renderTier();
    refreshFresh();
  }

  function startTicks(): void {
    if (freshTimer === undefined) freshTimer = setInterval(refreshFresh, TICK_MS);
  }

  /** Pauses everything: the freshness ticker, a pending frame and its timers. */
  function stopTicks(): void {
    if (freshTimer !== undefined) clearInterval(freshTimer);
    freshTimer = undefined;
    clearTimeout(frameTimer);
    frameTimer = undefined;
    cancelAnimationFrame(frameRaf);
    frameRaf = 0;
    pendingFrame = null;
  }

  function refreshFresh(): void {
    const live = liveOn();
    for (const handle of handles.values()) handle.fresh(freshMs, live);
  }

  /** A live frame for the stream on screen is kept; it is drawn now, or once the last draw is a second old. */
  function onLiveFrame(data: unknown): void {
    const frame = data as LiveFrame;
    if (!liveOn() || !isOnScreen(frame)) return;
    pendingFrame = frame;
    if (frameTimer !== undefined || frameRaf !== 0) return;
    const wait = Math.max(0, TICK_MS - (Date.now() - lastFrameAt));
    frameTimer = setTimeout(() => {
      frameTimer = undefined;
      frameRaf = requestAnimationFrame(applyPendingFrame);
    }, wait);
  }

  function isOnScreen(frame: LiveFrame): boolean {
    return frame.scope === state.scope && frame.tab === state.tab && frame.group === state.group && frame.range === state.range;
  }

  function applyPendingFrame(): void {
    frameRaf = 0;
    const frame = pendingFrame;
    pendingFrame = null;
    if (!frame || disposed) return;
    lastFrameAt = Date.now();
    freshMs = frame.t_ms;
    for (const p of frame.panels) handles.get(p.id)?.update(p);
    refreshFresh();
  }

  /** An alert fired: a toast, and a reload when the Alerts tab is open. */
  function onAlert(data: unknown): void {
    const alert = (data ?? {}) as { what?: string; text?: string };
    toast(`${alert.what ?? 'Alert'}: ${alert.text ?? ''}`);
    if (state.tab === 'alerts') void load();
  }

  // ---- Preferences: live, annotations, density, time shift ----

  function toggleLive(): void {
    state = { ...state, live: !state.live };
    savePrefs(state);
    renderControls();
    syncLive();
  }

  function toggleAnnotations(): void {
    state = { ...state, annotations: !state.annotations };
    savePrefs(state);
    renderControls();
    drawGrid();
  }

  function toggleDensity(): void {
    state = { ...state, compact: !state.compact };
    savePrefs(state);
    root.classList.toggle('mx-compact', state.compact);
    drawGrid();
  }

  /** Moves the window back (negative) or forward, never past now. */
  function shiftBy(delta: number): void {
    const shift = Math.min(0, state.shift + delta);
    if (shift === state.shift) return;
    navigate({ ...state, shift, zoom: null });
  }

  function toggleScopePin(): void {
    const wasPinned = saved.scopes.includes(state.scope);
    setSaved(togglePinnedScope(saved, state.scope));
    renderStar();
    renderTreeList();
    toast(wasPinned ? 'Scope unpinned' : 'Scope pinned');
  }

  function stopCompare(): void {
    navigate({ ...state, compare: 'prev' });
  }

  function onCompareChange(): void {
    if (compareSel.value === 'scope') {
      openCompare();
      compareSel.value = compareKey(state.compare);
      return;
    }
    navigate({ ...state, compare: compareSel.value });
  }

  // ---- Overlays: find, compare, settings ----

  function openFind(): void {
    palette(findItems(), 'Find a scope, tab or action…');
  }

  /** The scope list, where a pick compares the view with that scope. */
  function openCompare(): void {
    const items: PaletteItem[] = scopes.map((n) => ({
      group: n.group,
      label: n.label.trim(),
      run: () => navigate({ ...state, compare: 'scope:' + n.id }),
    }));
    palette(items, 'Compare with which scope?');
  }

  function findItems(): PaletteItem[] {
    const items: PaletteItem[] = [];
    for (const t of visibleTabs()) items.push({ group: 'tab', label: t.label, run: () => selectTab(t.key) });
    for (const n of scopes) items.push({ group: n.group, label: n.label.trim(), run: () => go({ scope: n.id }) });
    for (const r of RANGES) items.push({ group: 'range', label: 'Last ' + r.key, run: () => navigate({ ...state, range: r.key, zoom: null }) });
    items.push(
      { group: 'action', label: state.live ? 'Turn live off' : 'Turn live on', run: () => toggleLive() },
      { group: 'action', label: 'New alert rule', run: () => newRule() },
      { group: 'action', label: 'Compare with another scope', run: () => openCompare() },
      { group: 'action', label: (saved.scopes.includes(state.scope) ? 'Unpin' : 'Pin') + ' this scope', run: () => toggleScopePin() },
      { group: 'action', label: 'Reset zoom', run: () => resetZoom() },
      { group: 'action', label: 'Toggle annotations', run: () => toggleAnnotations() },
      { group: 'action', label: 'Toggle compact density', run: () => toggleDensity() },
      { group: 'action', label: 'Export this view (CSV)', run: () => exportLink.click() },
      { group: 'action', label: 'Keyboard shortcuts', run: () => help() },
    );
    return items;
  }

  async function openSettings(): Promise<void> {
    try {
      const current = await api.metricsSettings();
      if (!disposed) settingsDialog(current, scopes, (s) => api.metricsPutSettings(s));
    } catch (err) {
      toast(errorText(err), 'error');
    }
  }

  // ---- Keyboard ----

  /** The single-key shortcuts (the keys 1 to 9 pick a tab and are handled before this map). */
  const shortcuts = new Map<string, () => void>([
    ['/', () => openFind()],
    ['?', () => help()],
    ['l', () => toggleLive()],
    ['z', () => resetZoom()],
    ['f', () => toggleScopePin()],
    ['a', () => toggleAnnotations()],
    ['c', () => openCompare()],
    ['d', () => toggleDensity()],
    ['n', () => newRule()],
    ['[', () => shiftBy(-1)],
    [']', () => shiftBy(1)],
  ]);

  function onKey(e: KeyboardEvent): void {
    if (e.key === 'Escape') {
      onEscape();
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      openFind();
      return;
    }
    if (isTyping(e.target) || e.altKey || e.ctrlKey || e.metaKey || overlayOpen()) return;
    if (/^[1-9]$/.test(e.key)) {
      const tab = visibleTabs()[Number(e.key) - 1];
      if (tab) {
        e.preventDefault();
        selectTab(tab.key);
      }
      return;
    }
    const run = shortcuts.get(e.key);
    if (run) {
      e.preventDefault();
      run();
    }
  }

  /** Esc closes the open overlay, or else leaves a chart's full-screen view. */
  function onEscape(): void {
    if (overlayOpen()) {
      closeOverlay();
      return;
    }
    const full = root.querySelector('.mx-card.mx-full');
    if (full) {
      full.classList.remove('mx-full');
      requestAnimationFrame(redrawAll);
    }
  }

  function isTyping(target: EventTarget | null): boolean {
    return target instanceof HTMLElement && target.closest('input, select, textarea') !== null;
  }

  // ---- Start and stop ----

  const cleanup = (): void => {
    disposed = true;
    for (const off of subscriptions) off();
    window.removeEventListener('hashchange', onHashChange);
    document.removeEventListener('keydown', onKey);
    document.removeEventListener('visibilitychange', syncLive);
    resizeObs.disconnect();
    clearTimeout(resizeTimer);
    stopTicks();
    lease.stop();
    disposeHandles();
    hover.clear();
    if (overlayOpen()) closeOverlay();
  };

  root.classList.add('mx-view');
  root.classList.toggle('mx-compact', state.compact);
  replace(root, head, tierEl, bodyEl);
  renderControls();
  renderTreeList();
  resizeObs.observe(gridEl);
  subscriptions.push(on('metrics', onLiveFrame), on('metrics.alert', onAlert));
  window.addEventListener('hashchange', onHashChange);
  document.addEventListener('keydown', onKey);
  document.addEventListener('visibilitychange', syncLive);
  void loadSaved();
  void loadCatalog();
  void load();
  return cleanup;
}
