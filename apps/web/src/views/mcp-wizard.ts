import type { McpProbeTool } from '../api';
// The Add/Edit MCP server wizard (docs/design/mcp.md, "The console's configuration flow").
// CONTRACT (fixed; the step modules import these types). The shell and steps 1, 5, 6 are in this file;
// step 2 is mcp-wizard-secrets.ts, steps 3 and 4 are mcp-wizard-test.ts.
import { api, mcpApi } from '../api';
import type { McpPick, McpPreset, McpServerConfig, McpServerView, McpSetting, McpSpec, McpTransport } from '../api';
import { field, input } from '../ui/components';
import { h, replace } from '../ui/dom';
import { Role, roleLabel } from '../roles';
import { LIMIT_BOUNDS, MCP_ID_RE, blankConfig, btn, iconBtn, kv, limitsProblem, matchingPreset, pickText, specsOfBag, transportText } from './mcp-common';
import { THIS_HUB, errorText } from './vault-common';
import { step2Credentials } from './mcp-wizard-secrets';
import { step3Test, step4Tools } from './mcp-wizard-test';
import '../styles/proto/settings-b.css';

export type WizardStep = 1 | 2 | 3 | 4 | 5 | 6;

export interface WizardState {
  mode: 'create' | 'edit';
  /** The config being built. In edit mode it starts from the server's config; the id never changes. */
  draft: McpServerConfig;
  /** True once the server exists on the hub (after the first persist). */
  saved: boolean;
  /** The last view the hub returned for this server (null until saved). */
  server: McpServerView | null;
  /** The tools of the last passing test in this session (null until one passed). */
  probe: McpProbeTool[] | null;
  /** The step the owner is on. */
  step: WizardStep;
  /** The credential and setting specs of the preset chosen in step 1 (edit mode: from the catalog by transport, else the server's own settings). */
  specs: { env: McpSpec[]; headers: McpSpec[] };
  /**
   * True for a custom or remote server: its env or header rows are the owner's own, added and removed in step 2
   * (env for a container, headers for a remote URL). False for a preset, whose rows come from the catalogue.
   */
  ownRows: boolean;
}

export interface WizardCtx {
  state: WizardState;
  /** Moves to a step (steps already reached may be revisited; step 3 needs the server saved). */
  go(step: WizardStep): void;
  /**
   * Writes the draft to the hub: POST when not yet saved (created disabled), else PATCH the editable
   * fields. Updates `state.saved` and `state.server`. Returns the view, or undefined after showing the error.
   */
  persist(): Promise<McpServerView | undefined>;
  /** Re-renders the current step (after a change that alters another step's content). */
  redraw(): void;
  /** Closes the wizard. Asks first when the draft has unsaved changes. */
  close(): void;
}

const STEP_LABELS = ['Source', 'Credentials', 'Test', 'Tools', 'Access', 'Enable'];

/** Shell-only state: what the owner picked in step 1, and what the page has saved. */
interface Local {
  catalog: McpPreset[];
  catalogError: string;
  /** This hub first, then the live nodes. */
  nodes: { id: string; name: string }[];
  /** 'custom', 'remote' or `preset:<id>`. Null until the owner picks one (create mode). */
  source: string | null;
  loaded: boolean;
  /** The furthest step reached, so the owner can jump back to it. */
  reached: WizardStep;
  /** The draft as last saved (or as opened). Differs from the draft when there are unsaved changes. */
  baseline: string;
  persisted: boolean;
  error: string;
  busy: boolean;
  confirming: boolean;
}

/** Opens the wizard. `existing` null means Add server; a view means Edit (the same steps, prefilled). */
export function openWizard(existing: McpServerView | null, onDone: () => void, presetId: string | null = null): void {
  const state: WizardState = existing
    ? { mode: 'edit', draft: copyConfig(existing), saved: true, server: existing, probe: null, step: 1, specs: { env: [], headers: [] }, ownRows: false }
    : { mode: 'create', draft: blankConfig('', '', { kind: 'stdio', image: '' }), saved: false, server: null, probe: null, step: 1, specs: { env: [], headers: [] }, ownRows: false };
  const local: Local = {
    catalog: [], catalogError: '', nodes: [], source: null, loaded: false, reached: 1,
    baseline: JSON.stringify(state.draft), persisted: false, error: '', busy: false, confirming: false,
  };
  let open = true;

  const title = existing ? `Edit ${existing.name || existing.id}` : 'Add MCP server';
  const shell = sheetShell(title, () => requestClose());
  const backdrop = shell.scrim;
  const bodyEl = shell.body;
  const foot = shell.foot;

  const ctx: WizardCtx = {
    state,
    go(step) {
      state.step = step;
      local.reached = Math.max(local.reached, step) as WizardStep;
      local.error = '';
      renderAll();
    },
    persist,
    redraw: () => renderAll(),
    close: () => requestClose(),
  };

  function renderAll(): void {
    replace(
      bodyEl,
      h('div.mcpwz-head', null, iconBtn('x', () => requestClose(), 'Close')),
      h('div.mcpwz-split', null, stepList(), h('div.mcpwz-body', null, stepBody())),
    );
    renderFoot();
  }

  function stepList(): HTMLElement {
    return h(
      'ol.mcpwz-rail',
      { 'aria-label': 'Wizard steps' },
      STEP_LABELS.map((label, i) => {
        const n = (i + 1) as WizardStep;
        const cur = n === state.step;
        const b = h('button.mcpwz-step', { type: 'button', disabled: n > local.reached && !cur, 'aria-current': cur ? 'step' : null }, `${n}. ${label}`);
        if (cur) b.classList.add('on');
        else if (n < state.step) b.classList.add('done');
        b.addEventListener('click', () => ctx.go(n));
        return h('li', null, b);
      }),
    );
  }

  function stepBody(): Node[] {
    if (state.step === 1 && !local.loaded) return [h('p.muted', null, 'Loading the presets.')];
    switch (state.step) {
      case 1:
        return step1(ctx, local);
      case 2:
        return nodesOf(step2Credentials(ctx));
      case 3:
        return nodesOf(step3Test(ctx));
      case 4:
        return nodesOf(step4Tools(ctx));
      case 5:
        return step5(ctx, local);
      case 6:
        return step6(ctx);
    }
  }

  function renderFoot(): void {
    const step = state.step;
    const back = step > 1 ? btn('Back', () => ctx.go((step - 1) as WizardStep), { kind: 'ghost' }) : null;
    const primary =
      step < 6
        ? btn('Next', () => void next(), { kind: 'primary' })
        : btn(state.draft.enabled ? 'Enable and save' : 'Save', () => void finish(), { kind: 'primary' });
    for (const b of [back, primary]) if (b) b.disabled = local.busy;
    replace(foot, local.error ? h('p.mcpwz-err', { role: 'alert' }, local.error) : null, h('div.mcpwz-row', null, back, primary));
  }

  /** Writes the draft and reports whether it worked. The error shows in the footer. */
  async function saveFromUi(): Promise<boolean> {
    local.busy = true;
    renderFoot();
    const view = await persist();
    local.busy = false;
    renderFoot();
    return view !== undefined;
  }

  async function next(): Promise<void> {
    const step = state.step;
    if (step === 1) {
      const problem = sourceProblem(ctx, local);
      if (problem) {
        local.error = problem;
        renderFoot();
        return;
      }
    }
    // Next leaves steps 4 and 5 with a write; steps 1 to 3 never persist.
    if (step === 4 || step === 5) {
      if (!(await saveFromUi())) return;
    }
    ctx.go((step + 1) as WizardStep);
  }

  async function finish(): Promise<void> {
    if (!(await saveFromUi())) return;
    dispose();
  }

  async function persist(): Promise<McpServerView | undefined> {
    // Step 6 can be reached with a bad limit typed in step 1, so the check runs here too.
    const problem = limitsProblem(state.draft.limits);
    if (problem) {
      local.error = problem;
      renderFoot();
      return undefined;
    }
    const d = { ...state.draft, env: unsetRefsDropped(state.draft.env), headers: unsetRefsDropped(state.draft.headers) };
    try {
      if (!state.saved) {
        const created = await mcpApi.createServer({ ...d, enabled: false });
        state.saved = true;
        state.server = created;
        if (!d.enabled) return finishPersist(created);
      }
      const view = await mcpApi.patchServer(d.id, {
        name: d.name,
        enabled: d.enabled,
        transport: d.transport,
        env: d.env,
        headers: d.headers,
        tools: d.tools,
        access: d.access,
        limits: d.limits,
        allow_private: d.allow_private,
        storage: d.storage,
      });
      state.server = view;
      return finishPersist(view);
    } catch (err) {
      local.error = errorText(err);
      renderFoot();
      return undefined;
    }
  }

  function finishPersist(view: McpServerView): McpServerView {
    local.persisted = true;
    local.baseline = JSON.stringify(state.draft);
    local.error = '';
    return view;
  }

  function requestClose(): void {
    if (!open || local.confirming) return;
    if (JSON.stringify(state.draft) === local.baseline) {
      dispose();
      return;
    }
    askDiscard();
  }

  /**
   * The discard confirm, stacked over the wizard (the wizard stays underneath). Keep editing or Escape closes it;
   * the wizard's own Escape handler stands aside while it is open.
   */
  function askDiscard(): void {
    local.confirming = true;
    const confirm = sheetShell('Discard changes?', () => closeConfirm());
    confirm.body.append(h('p', null, 'The changes to this server are not saved.'));
    replace(
      confirm.foot,
      btn('Keep editing', () => closeConfirm(), { kind: 'ghost' }),
      btn('Discard', () => {
        closeConfirm();
        dispose();
      }, { kind: 'danger' }),
    );
    const onConfirmKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') closeConfirm();
    };
    function closeConfirm(): void {
      if (!local.confirming) return;
      local.confirming = false;
      document.removeEventListener('keydown', onConfirmKey);
      confirm.scrim.remove();
    }
    document.addEventListener('keydown', onConfirmKey);
    document.body.appendChild(confirm.scrim);
  }

  function onKey(e: KeyboardEvent): void {
    if (e.key !== 'Escape') return;
    // The discard confirm handles its own Escape.
    if (local.confirming) return;
    // A dialog opened from this one (Add secret) handles its own Escape.
    if (document.querySelectorAll('.scrim').length > 1) return;
    requestClose();
  }

  function dispose(): void {
    if (!open) return;
    open = false;
    document.removeEventListener('keydown', onKey);
    backdrop.remove();
    if (local.persisted) onDone();
  }

  document.body.appendChild(backdrop);
  document.addEventListener('keydown', onKey);
  renderAll();

  void Promise.all([
    mcpApi.catalog().catch((err: unknown): McpPreset[] => {
      local.catalogError = errorText(err);
      return [];
    }),
    api.nodes().catch(() => null),
  ]).then(([catalog, nodes]) => {
    if (!open) return;
    local.catalog = catalog;
    local.nodes = [
      { id: THIS_HUB, name: 'This hub' },
      ...(nodes ?? []).filter((n) => !n.revoked).map((n) => ({ id: n.id, name: n.name })),
    ];
    if (existing) {
      // By transport, not by id: an id can be typed by hand, and a custom server can carry a preset's id.
      const match = matchingPreset(state.draft.transport, catalog);
      state.specs = match ? { env: match.env, headers: match.headers } : { env: specsOfBag(state.draft.env), headers: specsOfBag(state.draft.headers) };
      state.ownRows = !match;
      local.source = matchSource(state.draft.transport, catalog);
    }
    if (!existing && presetId) {
      const preset = catalog.find((p) => p.id === presetId);
      if (preset) choosePreset(ctx, local, preset);
    }
    local.loaded = true;
    renderAll();
  });
}

/**
 * The prototype's sheet over a scrim: a header with the title and a close button, a body and a footer. The caller
 * fills the body and the footer. It does not remove other scrims, so a dialog can stack over the wizard.
 */
function sheetShell(title: string, onClose: () => void): { scrim: HTMLElement; body: HTMLElement; foot: HTMLElement } {
  const body = h('div.sheet-b');
  const foot = h('footer.sheet-f');
  const scrim = h(
    'div.scrim',
    null,
    h(
      'div.sheet',
      { role: 'dialog', 'aria-modal': 'true', 'aria-label': title },
      h('header.sheet-h', null, h('h2', null, title), iconBtn('x', onClose, 'Close')),
      body,
      foot,
    ),
  );
  return { scrim, body, foot };
}

/** A step's nodes: the step module's element, or a list of them. */
function nodesOf(r: Node | Node[]): Node[] {
  return Array.isArray(r) ? r : [r];
}

function copyConfig(v: McpServerView): McpServerConfig {
  return JSON.parse(
    JSON.stringify({
      id: v.id, name: v.name, enabled: v.enabled, transport: v.transport, env: v.env, headers: v.headers,
      tools: v.tools, access: v.access, limits: v.limits, allow_private: v.allow_private, storage: v.storage,
      created_ms: v.created_ms, updated_ms: v.updated_ms,
    }),
  ) as McpServerConfig;
}

/** Which step-1 option an existing server's transport belongs to. */
function matchSource(t: McpTransport, catalog: McpPreset[]): string {
  const hit = matchingPreset(t, catalog);
  if (hit) return `preset:${hit.id}`;
  return t.kind === 'stdio' ? 'custom' : 'remote';
}

/** An empty secret reference means no vault secret was chosen yet: it is not sent to the hub. */
function unsetRefsDropped(bag: Record<string, McpSetting>): Record<string, McpSetting> {
  return Object.fromEntries(Object.entries(bag).filter(([, s]) => !('secret' in s && s.secret === '')));
}

/**
 * Credentials and literal settings, seeded from a preset: secrets as empty references (nothing is set
 * until the owner picks a vault secret or creates one), the rest empty values.
 */
function seed(specs: McpSpec[]): Record<string, McpSetting> {
  const out: Record<string, McpSetting> = {};
  for (const s of specs) out[s.name] = s.secret ? { secret: '' } : { value: '' };
  return out;
}

/** The first problem with step 1, or '' when the owner can go on. */
function sourceProblem(ctx: WizardCtx, local: Local): string {
  const { state } = ctx;
  const d = state.draft;
  if (!local.source) return 'Pick a preset, a custom container image or a remote URL.';
  if (local.source === 'custom' && (d.transport.kind !== 'stdio' || !d.transport.image.trim())) return 'Enter a container image.';
  if (local.source === 'remote' && (d.transport.kind !== 'http' || !/^https:\/\/\S/.test(d.transport.url))) {
    return 'Enter a remote URL that starts with https://.';
  }
  if (state.mode === 'create' && !MCP_ID_RE.test(d.id)) {
    return 'The ID must be 2 to 32 characters: lower-case letters, digits and hyphens, starting with a letter.';
  }
  if (!d.name.trim()) return 'Enter a display name.';
  return limitsProblem(d.limits);
}

// ---- Step 1: source ----------------------------------------------------------

function step1(ctx: WizardCtx, local: Local): Node[] {
  const out: Node[] = [];
  if (local.catalogError) out.push(h('p.mcp-warn', null, `Presets did not load. ${local.catalogError}`));
  out.push(h('p.muted', null, 'Pick a preset, a custom container image or a remote URL.'));
  out.push(
    h(
      'div.mcpwz-srcs',
      null,
      local.catalog.map((p) => presetOption(ctx, local, p)),
      sourceOption(ctx, local, 'custom', 'Custom container image', 'Any image the hub can run, with an optional command.'),
      sourceOption(ctx, local, 'remote', 'Remote URL', 'A server the hub reaches over https.'),
    ),
  );
  if (local.source === 'custom') out.push(...customFields(ctx));
  if (local.source === 'remote') out.push(...remoteFields(ctx));
  out.push(...serverFields(ctx));
  out.push(advancedFields(ctx));
  return out;
}

/** One source choice: `button.mcpwz-src`, pressed when it is the chosen source. */
function srcButton(on: boolean, name: string, text: string, note: string | null, onPick: () => void): HTMLButtonElement {
  const b = h(
    'button.mcpwz-src',
    { type: 'button', 'aria-pressed': String(on) },
    h('strong', null, name),
    h('span.muted', null, text),
    note ? h('span.muted', null, note) : null,
  );
  if (on) b.classList.add('on');
  b.addEventListener('click', onPick);
  return b;
}

function presetOption(ctx: WizardCtx, local: Local, p: McpPreset): HTMLButtonElement {
  const note = p.status && p.status !== 'verified' ? p.note || `Not verified: ${p.status}` : null;
  return srcButton(local.source === `preset:${p.id}`, p.name, p.description, note, () => choosePreset(ctx, local, p));
}

function sourceOption(ctx: WizardCtx, local: Local, key: 'custom' | 'remote', title: string, text: string): HTMLButtonElement {
  return srcButton(local.source === key, title, text, null, () => {
    if (local.source === key) return;
    const d = ctx.state.draft;
    // Changing the source drops the settings the old one seeded or the owner added: a container takes env, a URL headers.
    ctx.state.specs = { env: [], headers: [] };
    d.env = {};
    d.headers = {};
    if (key === 'custom') {
      d.transport = d.transport.kind === 'stdio' ? d.transport : { kind: 'stdio', image: '' };
    } else {
      d.transport = d.transport.kind === 'http' ? d.transport : { kind: 'http', url: '' };
      // A volume applies to container servers only.
      d.storage = { volume: false };
    }
    ctx.state.ownRows = true;
    local.source = key;
    ctx.redraw();
  });
}

function choosePreset(ctx: WizardCtx, local: Local, p: McpPreset): void {
  const key = `preset:${p.id}`;
  if (local.source === key) return;
  const d = ctx.state.draft;
  local.source = key;
  d.transport =
    p.transport === 'stdio'
      ? { kind: 'stdio', image: p.image ?? '', ...(p.command.length ? { command: p.command } : {}) }
      : { kind: 'http', url: p.url ?? '' };
  ctx.state.specs = { env: p.env, headers: p.headers };
  ctx.state.ownRows = false;
  if (p.transport === 'http') d.storage = { volume: false };
  d.env = seed(p.env);
  d.headers = seed(p.headers);
  if (!d.id) d.id = p.id;
  if (!d.name) d.name = p.name;
  ctx.redraw();
}

function setStdio(ctx: WizardCtx, image: string, command: string): void {
  const prev = ctx.state.draft.transport;
  const workdir = prev.kind === 'stdio' ? prev.workdir : undefined;
  const parts = command.split(/\s+/).filter(Boolean);
  ctx.state.draft.transport = { kind: 'stdio', image, ...(parts.length ? { command: parts } : {}), ...(workdir ? { workdir } : {}) };
}

function customFields(ctx: WizardCtx): HTMLElement[] {
  const t = ctx.state.draft.transport;
  const imageIn = input({ type: 'text', value: t.kind === 'stdio' ? t.image : '', placeholder: 'ghcr.io/owner/server:1.0', autocomplete: 'off', spellcheck: 'false', 'aria-label': 'Container image' });
  const commandIn = input({
    type: 'text',
    value: t.kind === 'stdio' ? (t.command ?? []).join(' ') : '',
    placeholder: 'serve --stdio',
    autocomplete: 'off',
    spellcheck: 'false',
    'aria-label': 'Command',
  });
  const sync = (): void => setStdio(ctx, imageIn.value, commandIn.value);
  imageIn.addEventListener('input', sync);
  commandIn.addEventListener('input', sync);
  return [
    field('Container image', imageIn, 'The hub pulls and runs the image. Agents never see its credentials.'),
    field('Command', commandIn, 'Optional. Separate the arguments with spaces.'),
  ];
}

function remoteFields(ctx: WizardCtx): HTMLElement[] {
  const t = ctx.state.draft.transport;
  const urlIn = input({ type: 'text', value: t.kind === 'http' ? t.url : '', placeholder: 'https://example.com/mcp', autocomplete: 'off', spellcheck: 'false', 'aria-label': 'HTTPS URL' });
  urlIn.addEventListener('input', () => {
    ctx.state.draft.transport = { kind: 'http', url: urlIn.value };
  });
  return [field('HTTPS URL', urlIn, 'Must start with https://.')];
}

function serverFields(ctx: WizardCtx): HTMLElement[] {
  const d = ctx.state.draft;
  const isEdit = ctx.state.mode === 'edit';
  const idIn = input({ type: 'text', value: d.id, autocomplete: 'off', spellcheck: 'false', disabled: isEdit ? true : null, 'aria-label': 'ID' });
  idIn.addEventListener('input', () => {
    d.id = idIn.value.trim();
  });
  const nameIn = input({ type: 'text', value: d.name, autocomplete: 'off', 'aria-label': 'Display name' });
  nameIn.addEventListener('input', () => {
    d.name = nameIn.value;
  });
  return [
    field('ID', idIn, isEdit ? 'The ID cannot change.' : '2 to 32 characters: lower-case letters, digits and hyphens.'),
    field('Display name', nameIn),
  ];
}

/**
 * The Advanced part of step 1: the limits, private hosts and the storage volume. Closed until opened; the hub's
 * defaults are what a new server gets. A number that is not a whole number in range is named by sourceProblem.
 */
function advancedFields(ctx: WizardCtx): HTMLElement {
  const d = ctx.state.draft;
  const limitIn = (key: keyof McpServerConfig['limits'], label: string): HTMLInputElement => {
    const { lo, hi } = LIMIT_BOUNDS[key];
    const v = d.limits[key];
    const el = input({ type: 'number', min: lo, max: hi, step: 1, value: Number.isFinite(v) ? v : '', inputmode: 'numeric', 'aria-label': label });
    el.addEventListener('input', () => {
      d.limits[key] = el.valueAsNumber;
    });
    return el;
  };
  const privateBox = h('input', { type: 'checkbox' });
  privateBox.checked = d.allow_private;
  privateBox.addEventListener('change', () => {
    d.allow_private = privateBox.checked;
    ctx.redraw();
  });
  const volumeBox = h('input', { type: 'checkbox' });
  volumeBox.checked = d.storage.volume;
  volumeBox.addEventListener('change', () => {
    d.storage = { volume: volumeBox.checked };
  });
  const items: (Node | null)[] = [
    field('Timeout (seconds)', limitIn('timeout_s', 'Timeout (seconds)'), `Between ${LIMIT_BOUNDS.timeout_s.lo} and ${LIMIT_BOUNDS.timeout_s.hi}.`),
    field('Output size (KB)', limitIn('max_output_kb', 'Output size (KB)'), `Between ${LIMIT_BOUNDS.max_output_kb.lo} and ${LIMIT_BOUNDS.max_output_kb.hi}.`),
    field(
      'Idle stop (minutes)',
      limitIn('idle_stop_min', 'Idle stop (minutes)'),
      `Between ${LIMIT_BOUNDS.idle_stop_min.lo} and ${LIMIT_BOUNDS.idle_stop_min.hi}. The server stops after this long unused.`,
    ),
    h('label.check', null, privateBox, 'Allow private network hosts'),
    d.allow_private ? h('p.mcp-warn', null, 'This lets the server reach private network hosts and loopback addresses. Turn it on only for a host you run.') : null,
    d.transport.kind === 'stdio'
      ? h('label.check', null, volumeBox, 'Keep storage between runs (a volume)')
      : null,
    d.transport.kind === 'stdio' ? h('p.muted', null, 'The volume is mounted at /data. Deleting the server keeps it unless you choose to delete it too.') : null,
  ];
  return h('details.mcpwz-adv', null, h('summary', null, 'Advanced'), ...items);
}

// ---- Step 5: access ----------------------------------------------------------

function step5(ctx: WizardCtx, local: Local): Node[] {
  const d = ctx.state.draft;
  const roles = Object.values(Role).map((r) => ({ value: r, label: roleLabel(r) }));
  const nodes = local.nodes.map((n) => ({ value: n.id, label: n.name }));
  const noRoles = d.access.roles !== 'all' && d.access.roles.length === 0;
  const noNodes = d.access.nodes !== 'all' && d.access.nodes.length === 0;
  const out: Node[] = [
    h('p.muted', null, 'Only the roles and nodes picked here can reach this server.'),
    pickGroup('Roles', 'roles', d.access.roles, roles, (p) => {
      d.access.roles = p;
      ctx.redraw();
    }),
    pickGroup('Nodes', 'nodes', d.access.nodes, nodes, (p) => {
      d.access.nodes = p;
      ctx.redraw();
    }),
  ];
  if (noRoles || noNodes) out.push(h('p.mcp-warn', null, 'Nobody can use this server until a role and a node are picked.'));
  return out;
}

function pickGroup(
  legend: string,
  key: string,
  pick: McpPick,
  options: { value: string; label: string }[],
  onChange: (p: McpPick) => void,
): HTMLElement {
  const all = pick === 'all';
  const chosen = new Set<string>(all ? [] : pick);
  const groupName = `mcpwz-${key}`;
  const allRadio = h('input', { type: 'radio', name: groupName });
  allRadio.checked = all;
  allRadio.addEventListener('change', () => onChange('all'));
  const pickRadio = h('input', { type: 'radio', name: groupName });
  pickRadio.checked = !all;
  pickRadio.addEventListener('change', () => onChange([]));
  const checks = options.map((o) => {
    const cb = h('input', { type: 'checkbox' });
    cb.checked = chosen.has(o.value);
    cb.addEventListener('change', () => {
      if (cb.checked) chosen.add(o.value);
      else chosen.delete(o.value);
      onChange(options.filter((x) => chosen.has(x.value)).map((x) => x.value));
    });
    return h('label.check', null, cb, o.label);
  });
  return h(
    'fieldset.mcpwz-grp',
    null,
    h('legend', null, legend),
    h('div.mcpwz-row', null, h('label.check', null, allRadio, `All ${key}`), h('label.check', null, pickRadio, `Pick ${key}`)),
    all ? null : checks,
  );
}

// ---- Step 6: enable ----------------------------------------------------------

function step6(ctx: WizardCtx): Node[] {
  const d = ctx.state.draft;
  const settings = [...Object.values(d.env), ...Object.values(d.headers)];
  const secrets = settings.filter((s) => 'secret' in s).length;
  const rows: [string, string][] = [
    ['Name', d.name || '—'],
    ['ID', d.id],
    ['Transport', transportText(d.transport)],
    ['Secrets', String(secrets)],
    ['Settings', String(settings.length - secrets)],
    ['Tools', d.tools.allow === 'all' ? 'All tools' : `${d.tools.allow.length} picked`],
    ['Roles', pickText(d.access.roles)],
    ['Nodes', pickText(d.access.nodes)],
  ];
  const enabled = h('input', { type: 'checkbox' });
  enabled.checked = d.enabled;
  enabled.addEventListener('change', () => {
    d.enabled = enabled.checked;
    ctx.redraw();
  });
  return [
    kv(rows),
    h('label.check', null, enabled, 'Enabled'),
    h('p.muted', null, 'A disabled server stays configured and cannot run. Agents use it only when it is enabled, has a role and a node.'),
  ];
}
