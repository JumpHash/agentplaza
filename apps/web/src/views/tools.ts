// Setup > Tools: the connectors the plaza lends to agents. A key is written once
// here, kept in the plaza's vault, and never reaches an agent's VM.

import '../styles/tools.css';
import { api, type Connector, type ConnectorAction, type ConnectorCategory, type ConnectorTest } from '../api';
import { attempt, badge, brandLogo, button, empty, field, input, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

const CATEGORIES: [ConnectorCategory, string][] = [
  ['research', 'Research'],
  ['media', 'Media'],
  ['infra', 'Infra'],
  ['messaging', 'Messaging'],
  ['dev', 'Dev'],
];

const RISK: Record<ConnectorAction['risk'], Tone> = { read: 'ok', write: 'info', irreversible: 'danger' };

function categoryLabel(c: ConnectorCategory): string {
  return CATEGORIES.find(([key]) => key === c)?.[1] ?? c;
}

function splitList(text: string): string[] {
  return text.split(',').map((s) => s.trim()).filter(Boolean);
}

/** A connector's logo: its brand mark when it has one, else its initials. */
function logo(c: Connector): HTMLElement {
  if (c.brand) return brandLogo(c.brand, true);
  const letters = c.label.split(/\s+/).filter(Boolean).map((w) => w[0]).join('').slice(0, 2).toUpperCase();
  return h('span.logo.lg', { title: c.label }, h('span.mono-mark', null, letters));
}

function status(c: Connector): [string, Tone] {
  if (c.configured && c.enabled) return ['Enabled', 'ok'];
  if (c.configured) return ['Configured', 'info'];
  return ['Not configured', 'warn'];
}

export function mount(root: HTMLElement): Cleanup {
  let connectors: Connector[] = [];
  let filter: 'all' | ConnectorCategory = 'all';
  /** Last test result per connector; kept across re-renders until the next test. */
  const tests = new Map<string, ConnectorTest>();

  const filters = h('div.tool-filters');
  const grid = h('div.prov-grid');
  root.append(
    h('div.view-head', null, h('h1', null, 'Tools')),
    h('p.faint.tool-note', null, "Keys stay in the plaza's vault and never reach an agent's VM. Agents reach a tool only through the plaza."),
    filters,
    grid,
  );

  const refresh = async (): Promise<void> => {
    const list = await attempt(() => api.connectors());
    if (list) connectors = list;
    drawGrid();
  };

  /** Runs a change, reports it, then reloads so each card shows what the server now holds. */
  const act = async (run: () => Promise<unknown>, success: string): Promise<void> => {
    await attempt(run, success);
    await refresh();
  };

  const runTest = async (c: Connector): Promise<void> => {
    const report = await attempt(() => api.testConnector(c.id));
    if (!report) return;
    tests.set(c.id, report);
    drawGrid();
  };

  const drawFilters = (): void => {
    const chip = (key: 'all' | ConnectorCategory, text: string): HTMLElement => {
      const b = h('button.tool-filter', { type: 'button', 'aria-pressed': filter === key ? 'true' : 'false' }, text);
      b.addEventListener('click', () => {
        filter = key;
        drawFilters();
        drawGrid();
      });
      return b;
    };
    replace(filters, chip('all', 'All'), ...CATEGORIES.map(([key, text]) => chip(key, text)));
  };

  const drawGrid = (): void => {
    const shown = connectors.filter((c) => filter === 'all' || c.category === filter);
    replace(
      grid,
      ...(shown.length ? shown.map(card) : [empty('models', connectors.length ? 'Nothing in this category' : 'No tools yet')]),
    );
  };

  const card = (c: Connector): HTMLElement => {
    const [text, tone] = status(c);
    const result = tests.get(c.id);

    const toggle = h('input', { type: 'checkbox' }) as HTMLInputElement;
    toggle.checked = c.enabled;
    toggle.addEventListener('change', () => {
      const on = toggle.checked;
      void act(() => api.updateConnector(c.id, { enabled: on }), `${c.label} ${on ? 'enabled' : 'disabled'}`);
    });

    // Write-only: the key is typed here once and never read back.
    const keyInput = input({ type: 'password', autocomplete: 'new-password', placeholder: c.key_hint || 'API key' });
    const saveKey = button(
      'Save key',
      async () => {
        const value = keyInput.value.trim();
        if (!value) return;
        saveKey.disabled = true;
        await act(() => api.setConnectorSecret(c.id, value), 'Key saved');
      },
      { kind: 'primary', size: 'sm' },
    );
    keyInput.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') saveKey.click();
    });
    const clearKey = c.configured
      ? button('Clear key', () => void act(() => api.clearConnectorSecret(c.id), 'Key cleared'), { kind: 'ghost', size: 'sm' })
      : null;

    const rolesInput = input({ value: c.roles.join(', '), placeholder: 'all roles', autocomplete: 'off' });
    const saveRoles = button(
      'Save roles',
      () => void act(() => api.updateConnector(c.id, { roles: splitList(rolesInput.value) }), 'Roles saved'),
      { size: 'sm' },
    );

    const testBtn = button('Test', () => void runTest(c), { size: 'sm' });
    const resultBadge = result
      ? h(
          'span.tool-result',
          { title: result.detail },
          badge(result.ok ? `OK · ${result.latency_ms} ms · ${result.detail}` : `Failed · ${result.detail}`, result.ok ? 'ok' : 'danger', true),
        )
      : null;

    const docs = c.docs_url ? h('a.tool-docs', { href: c.docs_url, target: '_blank', rel: 'noopener noreferrer' }, 'Docs') : null;

    const actions = c.actions.length
      ? h(
          'ul.tool-actions',
          null,
          c.actions.map((a) => {
            const chip = badge(a.name, RISK[a.risk]);
            chip.title = a.risk === 'irreversible' ? `${a.description} · needs your approval` : a.description;
            return h('li', null, chip);
          }),
        )
      : null;

    return h(
      'article.prov-card.tool-card',
      null,
      h(
        'header.prov-head',
        null,
        logo(c),
        h(
          'div.prov-title',
          null,
          h('h3.truncate', null, c.label),
          h('span.faint.mono.truncate', { title: c.id }, `${c.id} · ${categoryLabel(c.category)}`),
        ),
      ),
      h(
        'div.prov-meta',
        null,
        badge(c.kind === 'cli' ? 'CLI' : 'API', c.kind === 'cli' ? 'accent' : 'muted'),
        badge(text, tone, true),
      ),
      h('p.tool-desc', null, c.description),
      actions,
      c.kind === 'cli' && c.cli
        ? h('p.faint.tool-cli', null, `Installs ${c.cli.binary} in the agent's VM, pointed at the plaza's gateway — no key inside the VM.`)
        : null,
      h('label.tool-toggle', null, toggle, h('span', null, 'Enabled')),
      field('API key', keyInput, c.configured ? 'Saved in the vault. Enter a new key to replace it.' : 'Write-only: never shown again after saving.'),
      h('div.prov-actions', null, saveKey, clearKey),
      field('Roles', rolesInput, 'Comma-separated; empty = all roles'),
      h('div.prov-actions', null, saveRoles),
      h('div.prov-actions', null, testBtn, resultBadge, h('span.prov-spacer'), docs),
    );
  };

  drawFilters();
  void refresh();
  return () => undefined;
}
