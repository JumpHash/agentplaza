// Setup > Tools: the connectors the plaza lends to agents, the connectors agents
// have proposed (waiting for the owner's review), and the tools agents
// have built and packaged into the plaza's library. A key is written once
// here, kept in the plaza's vault, and never reaches an agent's VM. Laid out like
// Models > Providers: grouped provider cards, and a Configure modal.

import '../styles/models.css';
import '../styles/tools.css';
import { api, type Connector, type ConnectorAction, type ConnectorField, type ConnectorProposal, type ConnectorTest, type Discovery, type LibraryTool } from '../api';
import { attempt, badge, brandLogo, button, dot, empty, field, input, modal, select, textarea, toast, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { on } from '../store';
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

const LIBRARY_STATE: Record<LibraryTool['state'], [string, Tone]> = {
  private: ['Private', 'muted'],
  pending: ['Pending review', 'warn'],
  published: ['Published', 'ok'],
  rejected: ['Rejected', 'danger'],
};

const PROPOSAL_STATE: Record<ConnectorProposal['state'], [string, Tone]> = {
  pending: ['Pending', 'warn'],
  approved: ['Approved', 'ok'],
  rejected: ['Rejected', 'danger'],
};

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
  if (f.source === 'sealed') return 'Vault sealed';
  if (f.source === 'out_of_scope') return 'Not available on this node';
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

/** A proposal's auth in one phrase: its kind, and the header, query or path name when it has one. */
function authLabel(s: ConnectorProposal['spec']): string {
  return s.auth.name ? `${s.auth.kind} ${s.auth.name}` : s.auth.kind;
}

/** An action's full URL: the proposed base with the action's path. */
function fullUrl(baseUrl: string, path: string): string {
  return baseUrl.replace(/\/+$/, '') + path;
}

/** The plaza's check failures, shown above a proposal whose approve is off. */
function checksBox(p: ConnectorProposal): HTMLElement {
  return h('div.stack', null, badge('Checks failed · approve is off', 'warn'), h('ul', null, p.checks.map((c) => h('li', { 'data-key': `check-${c}` }, c))));
}

export function mount(root: HTMLElement): Cleanup {
  let connectors: Connector[] = [];
  let library: LibraryTool[] = [];
  let discoveries: Discovery[] = [];
  let proposals: ConnectorProposal[] = [];
  let showDecided = false;
  const body = h('div.prov-body', null, h('p.faint', null, 'Loading tools…'));
  const propBody = h('div.prov-body', null);
  const libBody = h('div.prov-body', null, h('p.faint', null, 'Loading agent tools…'));
  const discBody = h('div.prov-body', null, h('p.faint', null, 'Loading found tools…'));
  root.append(
    h('div.view-head', null, h('h1', null, 'Tools')),
    h('div.stack', null, h('p.faint', null, "Keys stay in the plaza's vault; agents reach a tool only through the plaza."), body, propBody, libBody, discBody),
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
              { 'data-key': `act-${a.name}` },
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
      { 'data-key': `conn-${c.id}` },
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
        { 'data-key': `field-${f.name}` },
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

  // ---- Proposed connectors --------------------------------------------------

  const refreshProposals = async (): Promise<void> => {
    const list = await attempt(() => api.connectorProposals());
    if (list) proposals = list;
    renderProposals();
  };

  /** A decision changes both the proposals and, on approval, the connector list. */
  const refreshAfterDecision = async (): Promise<void> => {
    await refreshProposals();
    await refresh();
  };

  const renderProposals = (): void => {
    const pending = proposals.filter((p) => p.state === 'pending');
    const decided = proposals.filter((p) => p.state !== 'pending');
    if (!pending.length && !decided.length) {
      replace(propBody);
      return;
    }
    const parts: HTMLElement[] = [h('h2.prov-group-title', null, 'Proposed connectors')];
    if (pending.length) parts.push(h('div.prov-grid', null, pending.map(proposalCard)));
    else parts.push(h('p.faint', null, 'Nothing waiting for your review.'));
    if (decided.length) {
      parts.push(
        h(
          'div.prov-row',
          null,
          button(
            showDecided ? 'Hide decided' : `Decided (${decided.length})`,
            () => {
              showDecided = !showDecided;
              renderProposals();
            },
            { size: 'sm', kind: 'ghost' },
          ),
        ),
      );
      if (showDecided) parts.push(h('ul.lib-files', null, decided.map(decidedRow)));
    }
    replace(propBody, ...parts);
  };

  const proposalCard = (p: ConnectorProposal): HTMLElement => {
    const s = p.spec;
    const blocked = p.checks.length > 0;
    return h(
      'article.prov-card',
      { 'data-key': `prop-${p.id}` },
      h(
        'header.prov-head',
        null,
        h(
          'div.prov-title',
          null,
          h('h3.truncate', null, s.label),
          h('span.mono.faint.truncate', { title: s.id }, s.id),
        ),
      ),
      h(
        'div.prov-meta',
        null,
        h('span.faint', null, `proposed by ${p.proposer}`),
        badge(s.category, 'accent'),
        badge(authLabel(s), 'muted'),
      ),
      h('p.faint', { title: s.description }, s.description),
      h('p.mono.truncate', { title: s.base_url }, s.base_url),
      s.fields.length
        ? h(
            'ul.tool-actions',
            null,
            s.fields.map((f) =>
              h('li', { 'data-key': `pfield-${f.env}` }, h('span.tool-chip', { title: `$${f.env}${f.secret ? ' · secret' : ''}${f.hint ? ` · ${f.hint}` : ''}` }, f.required ? `${f.label} *` : f.label)),
            ),
          )
        : null,
      h(
        'ul.tool-actions',
        null,
        s.actions.map((a) =>
          h(
            'li',
            { 'data-key': `pact-${a.name}` },
            h(
              'span.tool-chip',
              { title: a.description },
              dot(RISK[a.risk]),
              a.name,
              h('span.mono', null, ` ${a.method} ${a.path}`),
            ),
          ),
        ),
      ),
      blocked ? checksBox(p) : null,
      h('div.prov-actions', null, button('Review', () => openReview(p), { size: 'sm', kind: 'primary' })),
    );
  };

  const decidedRow = (p: ConnectorProposal): HTMLElement => {
    const [text, tone] = PROPOSAL_STATE[p.state];
    return h(
      'li',
      { 'data-key': `decided-${p.id}` },
      h('span.mono', null, p.spec.id),
      ' ',
      badge(text, tone, true),
      h('span.faint', null, ` ${p.spec.label} · proposed by ${p.proposer}`),
      p.note ? h('span.faint', null, ` · ${p.note}`) : null,
    );
  };

  /** Review: the full spec, then Approve or Reject (Reject takes an optional note). */
  const openReview = (p: ConnectorProposal): void => {
    let close = () => {};
    const s = p.spec;
    const blocked = p.checks.length > 0;
    const note = input({ autocomplete: 'off', placeholder: 'Why it is rejected (optional)' });

    const decide = async (approve: boolean): Promise<void> => {
      approveBtn.disabled = true;
      rejectBtn.disabled = true;
      const reason = note.value.trim() || undefined;
      const done = await attempt(async () => {
        await api.reviewConnectorProposal(p.id, approve, approve ? undefined : reason);
        return true;
      }, `${s.label} ${approve ? 'approved' : 'rejected'}`);
      approveBtn.disabled = blocked;
      rejectBtn.disabled = false;
      if (!done) return;
      close();
      await refreshAfterDecision();
    };
    const approveBtn = button('Approve', () => void decide(true), { kind: 'primary' });
    const rejectBtn = button('Reject', () => void decide(false), { kind: 'ghost' });
    approveBtn.disabled = blocked;

    close = modal(
      `Review ${s.label}`,
      h(
        'div.stack',
        null,
        h('p.mono', null, `${s.id} · ${s.category}`),
        h('p.faint', null, `Proposed by ${p.proposer} · auth ${authLabel(s)}`),
        h('p.faint', null, s.description),
        h('p.mono', null, s.base_url),
        s.docs_url && /^https?:\/\//.test(s.docs_url) ? h('a', { href: s.docs_url, target: '_blank', rel: 'noopener noreferrer' }, 'Docs') : null,
        s.fields.length
          ? h(
              'p.faint',
              null,
              `Fields: ${s.fields.map((f) => `${f.label}${f.required ? ' (required)' : ''} · $${f.env}${f.secret ? ' · secret' : ''}`).join('; ')}`,
            )
          : null,
        blocked ? checksBox(p) : null,
        s.actions.map((a) =>
          h(
            'section.stack',
            { 'data-key': `review-${a.name}` },
            h('div.prov-row', null, dot(RISK[a.risk]), h('strong', null, a.name), h('span.faint', null, a.risk)),
            h('p.mono', null, `${a.method} ${fullUrl(s.base_url, a.path)}`),
            h('p.faint', null, a.description),
            h('pre.lib-file', null, JSON.stringify(a.input_schema, null, 2)),
          ),
        ),
        field('Note for the proposer', note, 'Optional. Only sent when rejecting.'),
        h('div.prov-row', null, h('span.prov-spacer'), rejectBtn, approveBtn),
      ),
    );
  };

  // ---- Made by agents -------------------------------------------------------

  const refreshLibrary = async (): Promise<void> => {
    const list = await attempt(() => api.library());
    if (list) library = list;
    renderLibrary();
  };

  const renderLibrary = (): void => {
    const heading = h('h2.prov-group-title', null, 'Made by agents');
    if (!library.length) {
      replace(libBody, heading, empty('models', 'Agents can build tools in their VM and package them with tool_package; they show up here.'));
      return;
    }
    const pending = library.filter((t) => t.state === 'pending');
    const rest = library.filter((t) => t.state !== 'pending');
    const groups: HTMLElement[] = [];
    if (pending.length) {
      groups.push(h('section.prov-group', null, h('h2.prov-group-title', null, 'Waiting for your review'), h('div.prov-grid', null, pending.map(libCard))));
    }
    if (rest.length) {
      groups.push(h('section.prov-group', null, h('h2.prov-group-title', null, 'Library'), h('div.prov-grid', null, rest.map(libCard))));
    }
    replace(libBody, heading, ...groups);
  };

  const libCard = (t: LibraryTool): HTMLElement => {
    const [text, tone] = LIBRARY_STATE[t.state];
    const actions: HTMLElement[] = [button('Files', () => openFiles(t), { size: 'sm' })];
    if (t.state === 'pending') {
      actions.push(
        button('Publish', () => void publish(t), { size: 'sm', kind: 'primary' }),
        button('Reject', () => openReject(t), { size: 'sm', kind: 'ghost' }),
      );
    }
    actions.push(button('Remove', () => openRemove(t), { size: 'sm', kind: 'ghost' }));

    return h(
      'article.prov-card',
      { 'data-key': `lib-${t.name}@${t.version}` },
      h(
        'header.prov-head',
        null,
        h(
          'div.prov-title',
          null,
          h('h3.mono.truncate', null, `x_${t.name}`),
          h('span.faint.truncate', { title: t.description }, t.description),
        ),
      ),
      h(
        'div.prov-meta',
        null,
        h('span.faint', null, `by ${t.author} · v${t.version}`),
        badge(text, tone, true),
        h('span.faint', null, `${t.calls} ${t.calls === 1 ? 'call' : 'calls'}`),
      ),
      h('p.lib-command', { title: 'Command' }, t.command.join(' ')),
      t.requires.length
        ? h('ul.tool-actions', null, t.requires.map((p) => h('li', { 'data-key': `req-${p}` }, h('span.tool-chip', null, p))))
        : null,
      t.review_note ? h('p.faint', null, `Note: ${t.review_note}`) : null,
      h('div.prov-actions', null, ...actions),
    );
  };

  const openFiles = (t: LibraryTool): void => {
    const view = h('pre.lib-file', null, 'Pick a file to see its content.');
    const list = t.files.length
      ? h(
          'ul.lib-files',
          null,
          t.files.map((f) =>
            h('li', { 'data-key': `file-${f.path}` }, button(`${f.path} · ${f.size} B`, () => void showFile(t, f.path, view), { size: 'sm', kind: 'ghost' })),
          ),
        )
      : h('p.faint', null, 'No files.');
    modal(`x_${t.name} · v${t.version}`, h('div.stack', null, list, view));
  };

  const showFile = async (t: LibraryTool, path: string, view: HTMLElement): Promise<void> => {
    view.textContent = 'Loading…';
    const file = await attempt(() => api.libraryFile(t.name, t.version, path));
    view.textContent = file ? file.content : 'Could not read this file.';
  };

  const publish = async (t: LibraryTool): Promise<void> => {
    const done = await attempt(async () => {
      await api.reviewLibraryTool(t.name, t.version, true);
      return true;
    }, `x_${t.name} published`);
    if (done) await refreshLibrary();
  };

  const openReject = (t: LibraryTool): void => {
    let close = () => {};
    const note = input({ autocomplete: 'off', placeholder: 'Why it is rejected (short)' });
    const reject = button(
      'Reject',
      async () => {
        reject.disabled = true;
        const done = await attempt(async () => {
          await api.reviewLibraryTool(t.name, t.version, false, note.value.trim() || undefined);
          return true;
        }, `x_${t.name} rejected`);
        reject.disabled = false;
        if (!done) return;
        close();
        await refreshLibrary();
      },
      { kind: 'primary' },
    );
    close = modal(
      `Reject x_${t.name}`,
      h(
        'div.stack',
        null,
        field('Note for the author', note, 'Optional. The author sees it.'),
        h('div.prov-row', null, h('span.prov-spacer'), reject),
      ),
    );
  };

  const openRemove = (t: LibraryTool): void => {
    let close = () => {};
    const remove = button(
      'Remove',
      async () => {
        remove.disabled = true;
        const done = await attempt(async () => {
          await api.removeLibraryTool(t.name, t.version);
          return true;
        }, `x_${t.name} removed`);
        remove.disabled = false;
        if (!done) return;
        close();
        await refreshLibrary();
      },
      { kind: 'primary' },
    );
    close = modal(
      `Remove x_${t.name}?`,
      h(
        'div.stack',
        null,
        h('p.faint', null, `Removes v${t.version} and its files from the library. This cannot be undone.`),
        h('div.prov-row', null, h('span.prov-spacer'), button('Cancel', () => close(), { kind: 'ghost' }), remove),
      ),
    );
  };

  // ---- Found by agents ------------------------------------------------------

  const refreshToolbox = async (): Promise<void> => {
    const list = await attempt(() => api.toolbox());
    if (list) discoveries = list;
    renderToolbox();
  };

  const DISC_PAGE = 10;
  const DISC_KINDS = ['cli', 'library', 'service', 'api', 'technique'];
  const expanded = new Set<string>();
  let discQuery = '';
  let discKind = '';
  let discPage = 1;
  let discMounted = false;

  const discHeading = h('h2.prov-group-title', null, 'Found by agents');
  const discIntro = h('p.faint', null, 'Software, services and techniques agents found on the internet, tried in their VMs and recommend to each other.');
  const discSearch = h('input.disc-search', {
    type: 'search',
    placeholder: 'Search name, use, tags, by',
    'aria-label': 'Search found tools',
    oninput: () => {
      discQuery = discSearch.value;
      discPage = 1;
      paintDiscoveries();
    },
  });
  const discKindSelect = select([{ value: '', label: 'All kinds' }, ...DISC_KINDS.map((k) => ({ value: k, label: k }))], '');
  discKindSelect.classList.add('disc-kind');
  discKindSelect.addEventListener('change', () => {
    discKind = discKindSelect.value;
    discPage = 1;
    paintDiscoveries();
  });
  const discCount = h('span.disc-count', null);
  const discToolbar = h('div.disc-toolbar', null, discSearch, discKindSelect, discCount);
  const discArea = h('div.disc-area', null);

  const matches = (d: Discovery, q: string): boolean => !q || [d.name, d.what_for, d.by, ...d.tags].some((s) => s.toLowerCase().includes(q));

  const act = (label: string, onClick: () => void, kind?: 'danger' | 'ghost'): HTMLButtonElement => {
    const b = button(label, onClick, { size: 'sm', kind });
    b.classList.add('disc-btn');
    return b;
  };

  const flagDisc = (d: Discovery): void => {
    let close = () => {};
    const reason = textarea({ rows: 3, placeholder: 'What is wrong with it?' });
    const save = button(
      'Save',
      async () => {
        const text = reason.value.trim();
        if (!text) {
          toast('Write a reason first', 'error');
          return;
        }
        save.disabled = true;
        const done = await attempt(() => api.flagDiscovery(d.id, text), `${d.name} flagged`);
        save.disabled = false;
        if (!done) return;
        close();
        await refreshToolbox();
      },
      { kind: 'primary' },
    );
    close = modal(
      `Flag ${d.name}`,
      h(
        'div.stack',
        null,
        field('Reason', reason, 'Shown next to the entry.'),
        h('div.prov-row', null, h('span.prov-spacer'), button('Cancel', () => close(), { kind: 'ghost' }), save),
      ),
    );
  };

  const unflagDisc = async (d: Discovery): Promise<void> => {
    const done = await attempt(() => api.flagDiscovery(d.id, null), `${d.name} unflagged`);
    if (done) await refreshToolbox();
  };

  const removeDisc = async (d: Discovery): Promise<void> => {
    if (!window.confirm(`Remove ${d.name} from the found tools?`)) return;
    const done = await attempt(() => api.removeDiscovery(d.id), `${d.name} removed`);
    if (!done) return;
    expanded.delete(d.id);
    await refreshToolbox();
  };

  const discRow = (d: Discovery): HTMLElement => {
    const open = expanded.has(d.id);
    const toggle = (): void => {
      if (expanded.has(d.id)) expanded.delete(d.id);
      else expanded.add(d.id);
      paintDiscoveries();
    };
    const flag = d.flagged ? badge('flagged', 'danger') : null;
    if (flag) flag.title = d.flagged ?? '';
    return h(
      'li.disc-row',
      { 'data-key': `disc-${d.id}` },
      h(
        'div.disc-main',
        null,
        h('div.disc-line', null, h('strong.disc-name', { title: d.name }, d.name), h('span.tool-chip', null, d.kind), flag),
        h(
          'div.disc-line',
          null,
          h('span.disc-what', { title: d.what_for }, d.what_for),
          h('span.disc-by', null, `by ${d.by} · ${d.endorsed_by.length} endorsements`),
        ),
      ),
      h(
        'div.disc-actions',
        null,
        act('Details', toggle),
        act(d.flagged ? 'Unflag' : 'Flag', () => {
          if (d.flagged) void unflagDisc(d);
          else flagDisc(d);
        }),
        act('Remove', () => void removeDisc(d), 'danger'),
      ),
      open
        ? h(
            'div.disc-details',
            null,
            h('pre.lib-file', null, d.how),
            h('p', null, d.verdict),
            d.tags.length ? h('ul.tool-actions', null, d.tags.map((t) => h('li', { 'data-key': `tag-${t}` }, h('span.tool-chip', null, t)))) : null,
            // Agents write these URLs, so only web links become anchors (as the proposal review does).
            d.url && /^https?:\/\//.test(d.url) ? h('a', { href: d.url, target: '_blank', rel: 'noopener noreferrer' }, d.url) : null,
          )
        : null,
    );
  };

  const paintDiscoveries = (): void => {
    const q = discQuery.trim().toLowerCase();
    const rows = discoveries.filter((d) => (!discKind || d.kind === discKind) && matches(d, q));
    const pages = Math.max(1, Math.ceil(rows.length / DISC_PAGE));
    discPage = Math.min(Math.max(1, discPage), pages);
    discCount.textContent = `${discoveries.length} found`;
    if (!discoveries.length) {
      replace(discArea, empty('models', 'Nothing yet. Agents record what they find with plaza_toolbox_share.'));
      return;
    }
    if (!rows.length) {
      replace(discArea, h('p.faint', null, 'Nothing matches that search.'));
      return;
    }
    const start = (discPage - 1) * DISC_PAGE;
    const prev = button(
      'Previous',
      () => {
        discPage -= 1;
        paintDiscoveries();
      },
      { kind: 'ghost', size: 'sm', disabled: discPage <= 1 },
    );
    const next = button(
      'Next',
      () => {
        discPage += 1;
        paintDiscoveries();
      },
      { kind: 'ghost', size: 'sm', disabled: discPage >= pages },
    );
    replace(
      discArea,
      h('ul.disc-list', null, rows.slice(start, start + DISC_PAGE).map(discRow)),
      h('div.disc-pager', null, prev, h('span', null, `page ${discPage} of ${pages}`), next),
    );
  };

  const renderToolbox = (): void => {
    if (!discMounted) {
      discMounted = true;
      replace(discBody, discHeading, discIntro, discToolbar, discArea);
    }
    paintDiscoveries();
  };

  void refresh();
  void refreshProposals();
  void refreshLibrary();
  void refreshToolbox();
  const offLibrary = on('library', () => {
    void refreshLibrary();
    void refreshToolbox();
  });
  const offConnector = on('connector', () => void refreshAfterDecision());
  const offToolbox = on('toolbox', () => void refreshToolbox());
  return () => {
    offLibrary();
    offConnector();
    offToolbox();
  };
}
