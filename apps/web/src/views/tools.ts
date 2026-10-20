// Settings > Tools: the connectors the plaza lends to agents, the connectors agents have proposed (waiting for the
// owner's review), the tools agents built and packaged into the plaza's library, and the tools agents found online.
// A key is written once here, kept in the plaza's vault, and never reaches an agent's VM.

import '../styles/proto/settings-a.css';
import { api, type Connector, type ConnectorAction, type ConnectorField, type ConnectorProposal, type ConnectorTest, type Discovery, type LibraryTool } from '../api';
import { attempt, badge, button, dot, empty, field, input, modal, select, textarea, toast, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { on } from '../store';
import type { Cleanup } from './index';
import { sheet } from './models-providers';

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

const DISC_PAGE = 10;
const DISC_KINDS = ['cli', 'library', 'service', 'api', 'technique'];

/** Every required field is read from the environment, so the vault holds none of the key. */
function fromEnvOnly(c: Connector): boolean {
  const required = c.fields.filter((f) => f.required);
  return required.length > 0 && required.every((f) => f.source === 'env');
}

/** A connector with no actions yet is coming soon; otherwise it needs a key while a required field is missing, else by whether it is on. */
function sectionOf(c: Connector): Section {
  if (!c.actions.length) return 'Coming soon';
  if (!c.configured || c.fields.some((f) => f.required && !f.set)) return 'Needs a key';
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

/** The tile letters: the first letter of the first two words of a name. */
function initials(s: string): string {
  return s.split(/\s+/).slice(0, 2).map((w) => w[0] || '').join('').toUpperCase();
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
function checksBanner(checks: string[]): HTMLElement {
  return h('div.mdl-banner', null, badge('Checks failed · approve is off', 'warn'), h('ul', null, checks.map((c) => h('li', { 'data-key': `check-${c}` }, c))));
}

/** A tile's letters: the connector's initials, in the prototype's tile. */
function tileOf(name: string): HTMLElement {
  return h('span.mdl-logo', { 'aria-hidden': 'true' }, initials(name));
}

export function mount(root: HTMLElement): Cleanup {
  let connectors: Connector[] = [];
  let library: LibraryTool[] = [];
  let discoveries: Discovery[] = [];
  let proposals: ConnectorProposal[] = [];
  let showDecided = false;

  const connBox = h('div.mdl-body');
  const propBox = h('div.mdl-body');
  const libBox = h('div.mdl-body');
  const discBox = h('div.mdl-body');
  root.append(
    h('div.mdl-page', null, h('div.view-head', null, h('div', null, h('h1', null, 'Tools'), h('p.mdl-faint', null, "Keys stay in the plaza's vault; agents reach a tool only through the plaza."))), connBox, propBox, libBox, discBox),
  );

  // ---- Connectors -----------------------------------------------------------------------------------------------

  const refresh = async (): Promise<void> => {
    const list = await attempt(() => api.connectors());
    if (list) connectors = list;
    render();
  };

  const render = (): void => {
    if (!connectors.length) {
      replace(connBox, empty('models', 'No tools yet', ''));
      return;
    }
    const groups: Record<Section, Connector[]> = { Enabled: [], Configured: [], 'Needs a key': [], 'Coming soon': [] };
    for (const c of connectors) groups[sectionOf(c)].push(c);
    const sections = SECTIONS.filter((s) => groups[s].length > 0).map((s) =>
      h('section.tl-group', { 'data-key': `grp-${s}` }, h('h2', null, s), h('div.mdl-grid', null, groups[s].map(card))),
    );
    replace(connBox, sections);
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

    const chips = c.actions.map((a) =>
      h(
        'span.tl-chip',
        { 'data-key': `act-${a.name}`, title: `${a.description}${a.risk === 'irreversible' ? ' · needs your approval' : ''}` },
        dot(RISK[a.risk]),
        h('span', null, a.name),
      ),
    );

    const foot =
      section === 'Coming soon'
        ? null
        : h(
            'div.mdl-row-a',
            null,
            button('Configure', () => openConfigure(c), { size: 'sm', kind: needsKey ? 'primary' : 'ghost' }),
            c.configured ? button('Test', () => void runTest(c), { size: 'sm', kind: 'ghost' }) : null,
            c.configured ? button(c.enabled ? 'Disable' : 'Enable', () => void setEnabled(c, !c.enabled), { size: 'sm', kind: 'ghost' }) : null,
          );

    return h(
      'article.mdl-card',
      { 'data-key': `conn-${c.id}` },
      h('div.mdl-card-h', null, tileOf(c.label), h('div.mdl-card-t', null, h('h3', null, c.label), h('p.mdl-sub', { title: c.description }, c.description))),
      h(
        'div.mdl-row-a',
        null,
        badge(c.kind === 'cli' ? 'CLI' : 'API', c.kind === 'cli' ? 'accent' : 'muted'),
        badge(text, tone, true),
        keyFromEnv ? badge('Key from env', 'info') : null,
      ),
      chips.length ? h('div.tl-chips', null, chips) : null,
      foot,
    );
  };

  /** Configure: one input per credential field, the roles, and enabled. Save writes each field typed into, then the settings. */
  const openConfigure = (c: Connector): void => {
    let close = (): void => {};
    const inputs = new Map<string, HTMLInputElement>();

    const fieldBlocks = c.fields.map((f) => {
      const control = f.secret
        ? input({ type: 'password', autocomplete: 'new-password', placeholder: f.hint || f.label })
        : input({ autocomplete: 'off', placeholder: f.hint || f.label });
      inputs.set(f.name, control);

      const remove =
        f.source === 'vault'
          ? h(
              'div',
              { style: 'display:flex;justify-content:flex-end;margin:4px 0 12px' },
              button(
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
              ),
            )
          : null;

      return h('div', { 'data-key': `field-${f.name}` }, field(`${f.label} · ${f.required ? 'required' : 'optional'}`, control, sourceLine(f)), remove);
    });

    const rolesInput = input({ value: c.roles.join(', '), placeholder: 'all roles', autocomplete: 'off' });
    const enabledBox = h('input', { type: 'checkbox', id: 'tl-f-enabled' }) as HTMLInputElement;
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

    close = sheet(
      `Configure ${c.label}`,
      h(
        'div.stack',
        null,
        c.cli ? h('p.mdl-faint', null, `Installs ${c.cli.binary} in the agent's VM, pointed at the plaza's gateway — no key inside the VM.`) : null,
        h('p.mdl-faint', null, 'Write-only: a stored value is never shown again. Empty fields keep what is stored.'),
        fieldBlocks,
        field('Roles', rolesInput, 'Comma-separated; empty = all roles'),
        h('label.check', null, enabledBox, h('span', null, 'Enabled')),
        c.docs_url ? h('p', null, h('a', { href: c.docs_url, target: '_blank', rel: 'noopener noreferrer' }, 'Docs')) : null,
      ),
      [save],
    );
  };

  // ---- Proposed connectors --------------------------------------------------------------------------------------

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
    if (!proposals.length) {
      replace(propBox);
      return;
    }
    const pending = proposals.filter((p) => p.state === 'pending');
    const decided = proposals.filter((p) => p.state !== 'pending');
    replace(
      propBox,
      h(
        'section.tl-group',
        { 'data-key': 'proposals' },
        h('h2', null, 'Proposed connectors'),
        pending.length ? h('div.mdl-grid', null, pending.map(proposalCard)) : h('p.mdl-faint', null, 'Nothing waiting for your review.'),
        h(
          'div.mdl-row-a',
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
        showDecided ? h('ul.tl-list', null, decided.map(decidedRow)) : null,
      ),
    );
  };

  const proposalCard = (p: ConnectorProposal): HTMLElement => {
    const s = p.spec;
    const blocked = p.checks.length > 0;
    return h(
      'article.mdl-card',
      { 'data-key': `prop-${p.id}` },
      h('div.mdl-card-h', null, tileOf(s.label), h('div.mdl-card-t', null, h('h3', null, s.label), h('p.tl-mono', { title: s.id }, s.id))),
      h('p.mdl-faint', { style: 'margin-top:8px' }, `proposed by ${p.proposer}`),
      h('div.tl-meta', null, badge(s.category, 'accent'), badge(authLabel(s), 'muted')),
      h('p.tl-trunc', { title: s.description, style: 'margin-top:8px' }, s.description),
      h('p.tl-mono', { title: s.base_url }, s.base_url),
      h(
        'div.tl-chips',
        null,
        s.fields.map((f) =>
          h(
            'span.tl-chip',
            { 'data-key': `pfield-${f.env}`, title: `$${f.env}${f.secret ? ' · secret' : ''}${f.hint ? ` · ${f.hint}` : ''}` },
            h('span', null, f.required ? `${f.label} *` : f.label),
          ),
        ),
      ),
      h(
        'div.tl-chips',
        null,
        s.actions.map((a) =>
          h(
            'span.tl-chip',
            { 'data-key': `pact-${a.name}`, title: a.description },
            dot(RISK[a.risk]),
            h('span', null, a.name, ' ', h('span.mdl-faint', null, `${a.method} ${a.path}`)),
          ),
        ),
      ),
      blocked ? h('div', { style: 'margin-top:12px' }, checksBanner(p.checks)) : null,
      h('div.mdl-row-a', null, button('Review', () => openReview(p), { size: 'sm', kind: 'primary' })),
    );
  };

  const decidedRow = (p: ConnectorProposal): HTMLElement => {
    const [text, tone] = PROPOSAL_STATE[p.state];
    return h(
      'li',
      { 'data-key': `decided-${p.id}` },
      h('span.tl-mono', null, p.spec.id),
      ' ',
      badge(text, tone, true),
      h('span.mdl-faint', null, ` ${p.spec.label} · proposed by ${p.proposer}`),
      p.note ? h('span.mdl-faint', null, ` · ${p.note}`) : null,
    );
  };

  /** Review: the full spec, then Approve or Reject (Reject takes an optional note). */
  const openReview = (p: ConnectorProposal): void => {
    let close = (): void => {};
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

    close = sheet(
      `Review ${s.label}`,
      h(
        'div.stack',
        null,
        h('p.tl-mono', null, `${s.id} · ${s.category}`),
        h('p.mdl-faint', null, `Proposed by ${p.proposer} · auth ${authLabel(s)}`),
        h('p', null, s.description),
        h('p.tl-mono', null, s.base_url),
        s.docs_url && /^https?:\/\//.test(s.docs_url) ? h('p', null, h('a', { href: s.docs_url, target: '_blank', rel: 'noopener noreferrer' }, 'Docs')) : null,
        s.fields.length
          ? h(
              'p.mdl-faint',
              null,
              `Fields: ${s.fields.map((f) => `${f.label}${f.required ? ' (required)' : ''} · $${f.env}${f.secret ? ' · secret' : ''}`).join('; ')}`,
            )
          : null,
        blocked ? checksBanner(p.checks) : null,
        s.actions.map((a) =>
          h(
            'div',
            { 'data-key': `review-${a.name}`, style: 'margin-top:12px' },
            h('div.tl-row', null, dot(RISK[a.risk]), h('strong', null, a.name), h('span.mdl-faint', null, a.risk)),
            h('p.tl-mono', null, `${a.method} ${fullUrl(s.base_url, a.path)}`),
            h('p.mdl-faint', null, a.description),
            h('pre.tl-pre', null, JSON.stringify(a.input_schema, null, 2)),
          ),
        ),
        field('Note for the proposer', note, 'Optional. Only sent when rejecting.'),
      ),
      [rejectBtn, approveBtn],
    );
  };

  // ---- Made by agents ---------------------------------------------------------------------------------------------

  const refreshLibrary = async (): Promise<void> => {
    const list = await attempt(() => api.library());
    if (list) library = list;
    renderLibrary();
  };

  const renderLibrary = (): void => {
    const heading = h('h2', null, 'Made by agents');
    if (!library.length) {
      replace(libBox, h('section.tl-group', { 'data-key': 'library' }, heading, h('p.mdl-faint', null, 'Agents can build tools in their VM and package them with tool_package; they show up here.')));
      return;
    }
    const pending = library.filter((t) => t.state === 'pending');
    const rest = library.filter((t) => t.state !== 'pending');
    replace(
      libBox,
      h(
        'section.tl-group',
        { 'data-key': 'library' },
        heading,
        pending.length ? [h('h3.mdl-h3', null, 'Waiting for your review'), h('div.mdl-grid', null, pending.map(libCard))] : null,
        rest.length ? [h('h3.mdl-h3', null, 'Library'), h('div.mdl-grid', null, rest.map(libCard))] : null,
      ),
    );
  };

  const libCard = (t: LibraryTool): HTMLElement => {
    const [text, tone] = LIBRARY_STATE[t.state];
    const actions: HTMLElement[] = [button('Files', () => openFiles(t), { size: 'sm', kind: 'ghost' })];
    if (t.state === 'pending') {
      actions.push(
        button('Publish', () => void publish(t), { size: 'sm', kind: 'primary' }),
        button('Reject', () => openReject(t), { size: 'sm', kind: 'ghost' }),
      );
    }
    actions.push(button('Remove', () => openRemove(t), { size: 'sm', kind: 'ghost' }));

    return h(
      'article.mdl-card',
      { 'data-key': `lib-${t.name}@${t.version}` },
      h('h3.tl-mono', { style: 'font-size:var(--fs);color:var(--ink)', title: `x_${t.name}` }, `x_${t.name}`),
      h('p.tl-trunc', { title: t.description }, t.description),
      h(
        'div.tl-meta',
        null,
        h('span.mdl-faint', null, `by ${t.author} · v${t.version}`),
        badge(text, tone, true),
        h('span.mdl-faint', null, `${t.calls} ${t.calls === 1 ? 'call' : 'calls'}`),
      ),
      h('p.tl-mono', { title: 'Command', style: 'margin-top:8px' }, t.command.join(' ')),
      t.requires.length
        ? h(
            'div.tl-chips',
            null,
            t.requires.map((r) => h('span.tl-chip', { 'data-key': `req-${r}` }, h('span', null, r))),
          )
        : null,
      t.review_note ? h('p.mdl-faint', { style: 'margin-top:8px' }, `Note: ${t.review_note}`) : null,
      h('div.mdl-row-a', null, ...actions),
    );
  };

  const openFiles = (t: LibraryTool): void => {
    const out = h('div', { id: 'tl-file-out' }, h('p.mdl-faint', null, 'Pick a file to see its content.'));
    const list = t.files.length
      ? t.files.map((f) => button(`${f.path} · ${f.size} B`, () => void showFile(t, f.path, out), { size: 'sm', kind: 'ghost' }))
      : [h('p.mdl-faint', null, 'No files.')];
    modal(`x_${t.name} · v${t.version}`, h('div', { style: 'display:flex;flex-direction:column;gap:8px;align-items:flex-start' }, list, out));
  };

  const showFile = async (t: LibraryTool, path: string, out: HTMLElement): Promise<void> => {
    replace(out, h('p.mdl-faint', null, 'Loading…'));
    const file = await attempt(() => api.libraryFile(t.name, t.version, path));
    replace(out, file ? h('pre.tl-pre', null, file.content) : h('p.mdl-faint', null, 'Could not read this file.'));
  };

  const publish = async (t: LibraryTool): Promise<void> => {
    const done = await attempt(async () => {
      await api.reviewLibraryTool(t.name, t.version, true);
      return true;
    }, `x_${t.name} published`);
    if (done) await refreshLibrary();
  };

  const openReject = (t: LibraryTool): void => {
    let close = (): void => {};
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
    close = sheet(`Reject x_${t.name}`, h('div.stack', null, field('Note for the author', note, 'Optional. The author sees it.')), [reject]);
  };

  const openRemove = (t: LibraryTool): void => {
    let close = (): void => {};
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
    close = sheet(`Remove x_${t.name}?`, h('p', null, `Removes v${t.version} and its files from the library. This cannot be undone.`), [
      button('Cancel', () => close(), { kind: 'ghost' }),
      remove,
    ]);
  };

  // ---- Found by agents ----------------------------------------------------------------------------------------------

  const expanded = new Set<string>();
  let discQuery = '';
  let discKind = '';
  let discPage = 1;
  let discMounted = false;

  const discSearch = input({ type: 'search', placeholder: 'Search found tools', 'aria-label': 'Search found tools', autocomplete: 'off' });
  const discKindSelect = select([{ value: '', label: 'All kinds' }, ...DISC_KINDS.map((k) => ({ value: k, label: k }))], '');
  discKindSelect.setAttribute('aria-label', 'Kind');
  const discCount = h('span.mdl-faint', null);
  const discArea = h('div');

  discSearch.addEventListener('input', () => {
    discQuery = discSearch.value;
    discPage = 1;
    paintDiscoveries();
  });
  discKindSelect.addEventListener('input', () => {
    discKind = discKindSelect.value;
    discPage = 1;
    paintDiscoveries();
  });

  const matches = (d: Discovery, q: string): boolean => !q || [d.name, d.what_for, d.by, ...d.tags].some((s) => s.toLowerCase().includes(q));

  const flagDisc = (d: Discovery): void => {
    let close = (): void => {};
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
    close = sheet(`Flag ${d.name}`, h('div.stack', null, field('Reason', reason, 'Shown next to the entry.')), [button('Cancel', () => close(), { kind: 'ghost' }), save]);
  };

  const unflagDisc = async (d: Discovery): Promise<void> => {
    const done = await attempt(() => api.flagDiscovery(d.id, null), `${d.name} unflagged`);
    if (done) await refreshToolbox();
  };

  const removeDisc = (d: Discovery): void => {
    let close = (): void => {};
    const remove = button(
      'Remove',
      async () => {
        const done = await attempt(() => api.removeDiscovery(d.id), `${d.name} removed`);
        if (!done) return;
        close();
        expanded.delete(d.id);
        await refreshToolbox();
      },
      { kind: 'danger' },
    );
    close = sheet(`Remove ${d.name} from the found tools?`, h('div'), [button('Cancel', () => close(), { kind: 'ghost' }), remove]);
  };

  const discRow = (d: Discovery): HTMLElement => {
    const open = expanded.has(d.id);
    const toggle = (): void => {
      if (expanded.has(d.id)) expanded.delete(d.id);
      else expanded.add(d.id);
      paintDiscoveries();
    };
    const flag = d.flagged ? h('span.badge-t.t-danger', { title: d.flagged }, 'flagged') : null;
    return h(
      'li.disc-row',
      { 'data-key': `disc-${d.id}` },
      h('div.tl-row', null, h('span.tl-name', { title: d.name }, d.name), h('span.tl-chip', null, h('span', null, d.kind)), flag),
      h('p.tl-trunc', { title: d.what_for, style: 'margin-top:4px' }, d.what_for),
      h('p.mdl-faint', { style: 'margin-top:4px' }, `by ${d.by} · ${d.endorsed_by.length} endorsements`),
      h(
        'div.mdl-row-a',
        null,
        button('Details', toggle, { size: 'sm', kind: 'ghost' }),
        button(d.flagged ? 'Unflag' : 'Flag', () => {
          if (d.flagged) void unflagDisc(d);
          else flagDisc(d);
        }, { size: 'sm', kind: 'ghost' }),
        button('Remove', () => removeDisc(d), { size: 'sm', kind: 'danger' }),
      ),
      open
        ? h(
            'div',
            null,
            h('pre.tl-pre', null, d.how),
            h('p', null, d.verdict),
            d.tags.length ? h('div.tl-chips', null, d.tags.map((t) => h('span.tl-chip', { 'data-key': `tag-${t}` }, h('span', null, t)))) : null,
            // Agents write these URLs, so only web links become anchors (as the proposal review does).
            d.url && /^https?:\/\//.test(d.url) ? h('p', null, h('a', { href: d.url, target: '_blank', rel: 'noopener noreferrer' }, d.url)) : null,
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
      replace(discArea, h('p.mdl-faint', null, 'Nothing yet. Agents record what they find with plaza_toolbox_share.'));
      return;
    }
    if (!rows.length) {
      replace(discArea, h('p.mdl-faint', null, 'Nothing matches that search.'));
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
      h('ul.tl-list', null, rows.slice(start, start + DISC_PAGE).map(discRow)),
      h('div.tl-pager', null, prev, h('span.mdl-faint', null, `page ${discPage} of ${pages}`), next),
    );
  };

  const refreshToolbox = async (): Promise<void> => {
    const list = await attempt(() => api.toolbox());
    if (list) discoveries = list;
    renderToolbox();
  };

  const renderToolbox = (): void => {
    if (!discMounted) {
      discMounted = true;
      replace(
        discBox,
        h(
          'section.tl-group',
          { 'data-key': 'found' },
          h('h2', null, 'Found by agents'),
          h('p.mdl-faint', null, 'Tools agents found online, recorded with plaza_toolbox_share.'),
          h('div.tl-toolbar', null, discSearch, discKindSelect, discCount),
          discArea,
        ),
      );
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
