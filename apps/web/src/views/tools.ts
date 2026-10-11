// Setup > Tools: the connectors the plaza lends to agents. A key is written once
// here, kept in the plaza's vault, and never reaches an agent's VM. Laid out like
// Models > Providers: grouped provider cards, and a Configure modal.

import '../styles/models.css';
import '../styles/tools.css';
import { api, type Connector, type ConnectorAction, type ConnectorField, type ConnectorTest } from '../api';
import { attempt, badge, brandLogo, button, dot, empty, field, input, modal, toast, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

type Section = 'Enabled' | 'Configured' | 'Needs a key' | 'Coming soon';
const SECTIONS: Section[] = ['Enabled', 'Configured', 'Needs a key', 'Coming soon'];

const STATUS: Record<Section, [string, Tone]> = {
  Enabled: ['Enabled', 'ok'],
  Configured: ['Key set', 'info'],
  'Needs a key': ['Needs a key', 'warn'],
  'Coming soon': ['Coming soon', 'muted'],
};

const RISK: Record<ConnectorAction['risk'], Tone> = { read: 'ok', write: 'info', irreversible: 'danger' };

/** A required field with no value anywhere: neither in the vault nor in the environment. */
function missingRequired(c: Connector): boolean {
  return c.fields.some((f) => f.required && !f.set);
}

/** Every required field is read from the environment, so the vault holds none of the key. */
function fromEnvOnly(c: Connector): boolean {
  const required = c.fields.filter((f) => f.required);
  return required.length > 0 && required.every((f) => f.source === 'env');
}

/** A connector with no actions yet is coming soon; otherwise it needs a key while a required field is missing, else by whether it is on. */
function sectionOf(c: Connector): Section {
  if (!c.actions.length) return 'Coming soon';
  if (!c.configured || missingRequired(c)) return 'Needs a key';
  return c.enabled ? 'Enabled' : 'Configured';
}

/** The status line under one credential field's input. */
function sourceLine(f: ConnectorField): string {
  if (f.source === 'vault') return 'Set in the vault';
  if (f.source === 'env') return `Read from $${f.env}`;
  return `Not set — or set $${f.env} in apid's keys file`;
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

export function mount(root: HTMLElement): Cleanup {
  let connectors: Connector[] = [];
  const body = h('div.prov-body', null, h('p.faint', null, 'Loading tools…'));
  root.append(
    h('div.view-head', null, h('h1', null, 'Tools')),
    h('div.stack', null, h('p.faint', null, "Keys stay in the plaza's vault; agents reach a tool only through the plaza."), body),
  );

  const refresh = async (): Promise<void> => {
    const list = await attempt(() => api.connectors());
    if (list) connectors = list;
    render();
  };

  const render = (): void => {
    if (!connectors.length) {
      replace(body, empty('models', 'No tools yet'));
      return;
    }
    const groups: Record<Section, Connector[]> = { Enabled: [], Configured: [], 'Needs a key': [], 'Coming soon': [] };
    for (const c of connectors) groups[sectionOf(c)].push(c);
    const sections = SECTIONS.filter((s) => groups[s].length > 0).map((s) =>
      h('section.prov-group', null, h('h2.prov-group-title', null, s), h('div.prov-grid', null, groups[s].map(card))),
    );
    replace(body, ...sections);
  };

  const setEnabled = async (c: Connector, on: boolean): Promise<void> => {
    await attempt(() => api.updateConnector(c.id, { enabled: on }), `${c.label} ${on ? 'enabled' : 'disabled'}`);
    await refresh();
  };

  const runTest = async (c: Connector): Promise<void> => {
    const report = await attempt<ConnectorTest>(() => api.testConnector(c.id));
    if (!report) return;
    if (report.ok) toast(`OK · ${report.latency_ms} ms · ${report.detail}`);
    else toast(report.detail || 'Test failed', 'error');
  };

  const card = (c: Connector): HTMLElement => {
    const section = sectionOf(c);
    const [text, tone] = STATUS[section];
    const needsKey = section === 'Needs a key';
    const keyFromEnv = section !== 'Coming soon' && fromEnvOnly(c);

    const footer =
      section === 'Coming soon'
        ? null
        : h(
            'div.prov-actions',
            null,
            button('Configure', () => openConfigure(c), needsKey ? { size: 'sm', kind: 'primary' } : { size: 'sm' }),
            c.configured ? button('Test', () => void runTest(c), { size: 'sm' }) : null,
            c.configured ? button(c.enabled ? 'Disable' : 'Enable', () => void setEnabled(c, !c.enabled), { size: 'sm', kind: 'ghost' }) : null,
          );

    const chips = c.actions.length
      ? h(
          'ul.tool-actions',
          null,
          c.actions.map((a) =>
            h(
              'li',
              null,
              h(
                'span.tool-chip',
                { title: a.risk === 'irreversible' ? `${a.description} · needs your approval` : a.description },
                dot(RISK[a.risk]),
                a.name,
              ),
            ),
          ),
        )
      : null;

    return h(
      'article.prov-card',
      null,
      h(
        'header.prov-head',
        null,
        logo(c),
        h(
          'div.prov-title',
          null,
          h('h3.truncate', null, c.label),
          h('span.faint.truncate', { title: c.description }, c.description),
        ),
      ),
      h(
        'div.prov-meta',
        null,
        badge(c.kind === 'cli' ? 'CLI' : 'API', c.kind === 'cli' ? 'accent' : 'muted'),
        badge(text, tone, true),
        keyFromEnv ? badge('Key from env', 'info') : null,
      ),
      chips,
      footer,
    );
  };

  /** Configure: one input per credential field, the roles, and enabled. Save writes each field typed into, then the settings. */
  const openConfigure = (c: Connector): void => {
    let close = () => {};
    const inputs = new Map<string, HTMLInputElement>();

    const fieldBlocks = c.fields.map((f) => {
      const control = f.secret
        ? input({ type: 'password', autocomplete: 'new-password', placeholder: f.hint || f.label })
        : input({ autocomplete: 'off', placeholder: f.hint || f.label });
      inputs.set(f.name, control);

      const remove =
        f.source === 'vault'
          ? button(
              'Remove',
              async () => {
                const cleared = await attempt(async () => {
                  await api.clearConnectorSecret(c.id, f.name);
                  return true;
                }, `${f.label} removed`);
                if (!cleared) return;
                close();
                await refresh();
              },
              { size: 'sm', kind: 'ghost' },
            )
          : null;

      return h(
        'div.stack',
        null,
        field(`${f.label} · ${f.required ? 'required' : 'optional'}`, control, sourceLine(f)),
        remove ? h('div.prov-row', null, h('span.prov-spacer'), remove) : null,
      );
    });

    const rolesInput = input({ value: c.roles.join(', '), placeholder: 'all roles', autocomplete: 'off' });
    const enabledBox = h('input', { type: 'checkbox' }) as HTMLInputElement;
    enabledBox.checked = c.enabled;

    const save = button(
      'Save',
      async () => {
        save.disabled = true;
        const saved = await attempt(async () => {
          for (const f of c.fields) {
            const value = inputs.get(f.name)?.value.trim() ?? '';
            if (value) await api.setConnectorSecret(c.id, value, f.name);
          }
          await api.updateConnector(c.id, { enabled: enabledBox.checked, roles: splitList(rolesInput.value) });
          return true;
        }, 'Saved');
        save.disabled = false;
        if (!saved) return;
        close();
        await refresh();
      },
      { kind: 'primary' },
    );
    for (const control of inputs.values()) {
      control.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') save.click();
      });
    }

    close = modal(
      `Configure ${c.label}`,
      h(
        'div.stack',
        null,
        c.cli ? h('p.faint', null, `Installs ${c.cli.binary} in the agent's VM, pointed at the plaza's gateway — no key inside the VM.`) : null,
        h('p.faint', null, 'Write-only: a stored value is never shown again. Empty fields keep what is stored.'),
        fieldBlocks,
        field('Roles', rolesInput, 'Comma-separated; empty = all roles'),
        h('label.tool-toggle', null, enabledBox, h('span', null, 'Enabled')),
        c.docs_url ? h('a', { href: c.docs_url, target: '_blank', rel: 'noopener noreferrer' }, 'Docs') : null,
        h('div.prov-row', null, h('span.prov-spacer'), save),
      ),
    );
  };

  void refresh();
  return () => undefined;
}
