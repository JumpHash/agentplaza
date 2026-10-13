import type { McpProbeTool } from '../api';
// The Add/Edit MCP server wizard (docs/design/mcp.md, "The console's configuration flow").
// CONTRACT (fixed; the step modules import these types). The shell and steps 1, 5, 6 are in this file;
// step 2 is mcp-wizard-secrets.ts, steps 3 and 4 are mcp-wizard-test.ts.
import { api, mcpApi } from '../api';
import type { McpPick, McpPreset, McpServerConfig, McpServerView, McpSetting, McpSpec, McpTransport } from '../api';
import { BRAND_ICONS } from '../ui/brands.gen';
import { brandLogo, button, field, input, modal } from '../ui/components';
import { h, icon, replace } from '../ui/dom';
import type { IconName } from '../ui/icons.gen';
import { Role, roleLabel } from '../roles';
import { MCP_ID_RE, blankConfig, pickText, transportText } from './mcp-common';
import { THIS_HUB, errorText } from './vault-common';
import { step2Credentials } from './mcp-wizard-secrets';
import { step3Test, step4Tools } from './mcp-wizard-test';
import '../styles/mcp-wizard.css';

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
  /** The credential and setting specs of the preset chosen in step 1 (edit mode: from the catalog by id, else empty). */
  specs: { env: McpSpec[]; headers: McpSpec[] };
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
    ? { mode: 'edit', draft: copyConfig(existing), saved: true, server: existing, probe: null, step: 1, specs: { env: [], headers: [] } }
    : { mode: 'create', draft: blankConfig('', '', { kind: 'stdio', image: '' }), saved: false, server: null, probe: null, step: 1, specs: { env: [], headers: [] } };
  const local: Local = {
    catalog: [], catalogError: '', nodes: [], source: null, loaded: false, reached: 1,
    baseline: JSON.stringify(state.draft), persisted: false, error: '', busy: false, confirming: false,
  };
  let open = true;

  const backdrop = h('div.modal-backdrop.mcpw-backdrop');
  const sheet = h('div.modal.mcpw-sheet', { role: 'dialog', 'aria-modal': 'true', 'aria-label': 'MCP server' });
  const head = h('header.card-head');
  const bodyEl = h('div.card-body.mcpw-body');
  const foot = h('footer.mcpw-foot');
  sheet.append(head, bodyEl, foot);
  backdrop.appendChild(sheet);

  const title = (): string => (state.mode === 'create' ? 'Add MCP server' : `Edit ${state.draft.name || state.draft.id}`);

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
    replace(head, h('h2', null, title()), button('', () => requestClose(), { kind: 'ghost', icon: 'x', title: 'Close' }));
    replace(bodyEl, stepList(), stepBody());
    renderFoot();
  }

  function stepList(): HTMLElement {
    return h(
      'ol.mcpw-steps',
      { 'aria-label': 'Wizard steps' },
      STEP_LABELS.map((label, i) => {
        const n = (i + 1) as WizardStep;
        const reachable = n <= local.reached && n !== state.step;
        const li = h('li');
        if (n === state.step) li.classList.add('on');
        else if (reachable) li.classList.add('done');
        const btn = h(
          'button',
          { type: 'button', disabled: !reachable, 'aria-current': n === state.step ? 'step' : null },
          h('span.mcpw-num', null, String(n)),
          h('span', null, label),
        );
        if (reachable) btn.addEventListener('click', () => ctx.go(n));
        li.appendChild(btn);
        return li;
      }),
    );
  }

  function stepBody(): HTMLElement {
    if (state.step === 1 && !local.loaded) return h('p.mcpw-lede', null, 'Loading the presets.');
    switch (state.step) {
      case 1:
        return step1(ctx, local);
      case 2:
        return step2Credentials(ctx);
      case 3:
        return step3Test(ctx);
      case 4:
        return step4Tools(ctx);
      case 5:
        return step5(ctx, local);
      case 6:
        return step6(ctx);
    }
  }

  function renderFoot(): void {
    const step = state.step;
    const back = step > 1 ? button('Back', () => ctx.go((step - 1) as WizardStep), { kind: 'ghost' }) : null;
    const primary =
      step < 6
        ? button('Next', () => void next(), { kind: 'primary' })
        : button(state.draft.enabled ? 'Enable and save' : 'Save', () => void finish(), { kind: 'primary' });
    for (const b of [back, primary]) if (b) b.disabled = local.busy;
    replace(foot, local.error ? h('p.mcpw-error', { role: 'alert' }, local.error) : null, h('div.mcpw-actions', null, back, primary));
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
    local.confirming = true;
    const discard = (): void => {
      closeConfirm();
      dispose();
    };
    const body = h(
      'div.mcpw-confirm',
      null,
      h('p', null, 'The changes to this server are not saved.'),
      h(
        'div.mcpw-actions',
        null,
        button('Keep editing', () => closeConfirm(), { kind: 'ghost' }),
        button('Discard', discard, { kind: 'danger' }),
      ),
    );
    const closeConfirm = modal('Discard changes?', body, () => {
      local.confirming = false;
    });
  }

  function onKey(e: KeyboardEvent): void {
    if (e.key !== 'Escape') return;
    // A dialog opened from this one (the confirm, or Add secret) handles its own Escape.
    if (document.querySelectorAll('.modal-backdrop').length > 1) return;
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
      const match = catalog.find((p) => p.id === state.draft.id);
      state.specs = match ? { env: match.env, headers: match.headers } : { env: [], headers: [] };
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
  const hit = catalog.find((p) => (t.kind === 'stdio' ? p.transport === 'stdio' && p.image === t.image : p.transport === 'http' && p.url === t.url));
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
  return '';
}

// ---- Step 1: source ----------------------------------------------------------

function step1(ctx: WizardCtx, local: Local): HTMLElement {
  const root = h('div.mcpw-step');
  if (local.catalogError) root.appendChild(h('p.mcpw-warn', null, `Presets did not load. ${local.catalogError}`));
  root.appendChild(h('p.mcpw-lede', null, 'Pick a preset, or bring your own container image or remote URL.'));
  const options = h('div.mcpw-options');
  for (const p of local.catalog) options.appendChild(presetOption(ctx, local, p));
  options.appendChild(sourceOption(ctx, local, 'custom', 'Custom container image', 'Any image the hub can run, with an optional command.', 'terminal'));
  options.appendChild(sourceOption(ctx, local, 'remote', 'Remote URL', 'A server the hub reaches over https.', 'link'));
  root.appendChild(options);
  if (local.source === 'custom') root.appendChild(customFields(ctx));
  if (local.source === 'remote') root.appendChild(remoteFields(ctx));
  root.appendChild(serverFields(ctx));
  return root;
}

function optionButton(on: boolean, lead: Node, title: string, text: string, note: string | null, onPick: () => void): HTMLButtonElement {
  const b = h(
    'button.mcpw-option',
    { type: 'button', 'aria-pressed': on ? 'true' : 'false' },
    h('span.mcpw-option-head', null, lead, h('strong', null, title)),
    h('span.mcpw-option-text', null, text),
    note ? h('span.mcpw-note', null, note) : null,
  );
  if (on) b.classList.add('on');
  b.addEventListener('click', onPick);
  return b;
}

function presetOption(ctx: WizardCtx, local: Local, p: McpPreset): HTMLButtonElement {
  const lead: Node = p.id in BRAND_ICONS ? brandLogo(p.id) : icon('terminal');
  const note = p.status && p.status !== 'verified' ? p.note || `Not verified: ${p.status}` : null;
  return optionButton(local.source === `preset:${p.id}`, lead, p.name, p.description, note, () => choosePreset(ctx, local, p));
}

function sourceOption(ctx: WizardCtx, local: Local, key: 'custom' | 'remote', title: string, text: string, iconName: IconName): HTMLButtonElement {
  return optionButton(local.source === key, icon(iconName), title, text, null, () => {
    if (local.source === key) return;
    const d = ctx.state.draft;
    if (local.source?.startsWith('preset:')) {
      // Leaving a preset drops the settings it seeded.
      ctx.state.specs = { env: [], headers: [] };
      d.env = {};
      d.headers = {};
    }
    if (key === 'custom') d.transport = d.transport.kind === 'stdio' ? d.transport : { kind: 'stdio', image: '' };
    else d.transport = d.transport.kind === 'http' ? d.transport : { kind: 'http', url: '' };
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

function customFields(ctx: WizardCtx): HTMLElement {
  const t = ctx.state.draft.transport;
  const imageIn = input({ type: 'text', value: t.kind === 'stdio' ? t.image : '', placeholder: 'ghcr.io/owner/server:1.0', autocomplete: 'off', spellcheck: 'false' });
  const commandIn = input({
    type: 'text',
    value: t.kind === 'stdio' ? (t.command ?? []).join(' ') : '',
    placeholder: 'serve --stdio',
    autocomplete: 'off',
    spellcheck: 'false',
  });
  const sync = (): void => setStdio(ctx, imageIn.value, commandIn.value);
  imageIn.addEventListener('input', sync);
  commandIn.addEventListener('input', sync);
  return h(
    'div.mcpw-fields',
    null,
    field('Container image', imageIn, 'The hub pulls and runs the image. Agents never see its credentials.'),
    field('Command', commandIn, 'Optional. Separate the arguments with spaces.'),
  );
}

function remoteFields(ctx: WizardCtx): HTMLElement {
  const t = ctx.state.draft.transport;
  const urlIn = input({ type: 'text', value: t.kind === 'http' ? t.url : '', placeholder: 'https://example.com/mcp', autocomplete: 'off', spellcheck: 'false' });
  urlIn.addEventListener('input', () => {
    ctx.state.draft.transport = { kind: 'http', url: urlIn.value };
  });
  return h('div.mcpw-fields', null, field('HTTPS URL', urlIn, 'Must start with https://.'));
}

function serverFields(ctx: WizardCtx): HTMLElement {
  const d = ctx.state.draft;
  const isEdit = ctx.state.mode === 'edit';
  const idIn = input({ type: 'text', value: d.id, autocomplete: 'off', spellcheck: 'false', disabled: isEdit ? true : null });
  idIn.addEventListener('input', () => {
    d.id = idIn.value.trim();
  });
  const nameIn = input({ type: 'text', value: d.name, autocomplete: 'off' });
  nameIn.addEventListener('input', () => {
    d.name = nameIn.value;
  });
  return h(
    'div.mcpw-fields',
    null,
    field('ID', idIn, isEdit ? 'The ID cannot change.' : '2 to 32 characters: lower-case letters, digits and hyphens.'),
    field('Display name', nameIn),
  );
}

// ---- Step 5: access ----------------------------------------------------------

function step5(ctx: WizardCtx, local: Local): HTMLElement {
  const d = ctx.state.draft;
  const roles = Object.values(Role).map((r) => ({ value: r, label: roleLabel(r) }));
  const nodes = local.nodes.map((n) => ({ value: n.id, label: n.name }));
  const noRoles = d.access.roles !== 'all' && d.access.roles.length === 0;
  const noNodes = d.access.nodes !== 'all' && d.access.nodes.length === 0;
  return h(
    'div.mcpw-step',
    null,
    h('p.mcpw-lede', null, 'Only the roles and nodes picked here can reach this server.'),
    pickGroup('Roles', 'roles', d.access.roles, roles, (p) => {
      d.access.roles = p;
      ctx.redraw();
    }),
    pickGroup('Nodes', 'nodes', d.access.nodes, nodes, (p) => {
      d.access.nodes = p;
      ctx.redraw();
    }),
    noRoles || noNodes ? h('p.mcpw-warn', null, 'Nobody can use this server until a role and a node are picked.') : null,
  );
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
  const groupName = `mcpw-${key}`;
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
    return h('label.mcpw-check', null, cb, h('span', null, o.label));
  });
  return h(
    'fieldset.mcpw-group',
    null,
    h('legend', null, legend),
    h('label.mcpw-check', null, allRadio, h('span', null, `All ${key}`)),
    h('label.mcpw-check', null, pickRadio, h('span', null, `Pick ${key}`)),
    all ? null : h('div.mcpw-checks', null, checks),
  );
}

// ---- Step 6: enable ----------------------------------------------------------

function step6(ctx: WizardCtx): HTMLElement {
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
  return h(
    'div.mcpw-step',
    null,
    h(
      'dl.mcpw-summary',
      null,
      rows.map(([k, v]) => h('div.mcpw-sum-row', null, h('dt', null, k), h('dd', null, v))),
    ),
    h('label.mcpw-check', null, enabled, h('span', null, 'Enabled')),
    h('p.mcpw-lede', null, 'A disabled server stays configured and cannot run. Agents use it only when it is enabled, has a role and a node.'),
  );
}
