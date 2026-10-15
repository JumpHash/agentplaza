// Setup > E2B API: the owner's keys for the hub's E2B gateway, the setup an SDK client needs, and the sandboxes the
// gateway runs (refreshed every 10 s while the screen is open). A key is shown once, in the reveal box, and is never
// stored: leaving the screen clears it. The pure helpers (the setup text, the actions a sandbox row offers, the memory
// text) are exported and unit-tested (test/tests.ts).

import '../styles/mcp.css';
import '../styles/mcp-hub.css';
import { e2bApi, type E2bKey, type E2bKeyCreated, type E2bSandbox } from '../api';
import { ago, attempt, badge, button, card, empty, field, input, modal, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { errorText } from './vault-common';

/** The hub machine's local sandbox gateway. An SDK client on the hub machine reaches the sandboxes through it. */
export const SANDBOX_GATEWAY = 'http://127.0.0.1:49982';
export const SANDBOX_REFRESH_MS = 10_000;

export type SandboxState = 'running' | 'paused';
export type SandboxAction = 'pause' | 'resume' | 'end';

/**
 * The setup an SDK client reads: the API under the hub's origin, the key, and the local sandbox gateway. `key` is the
 * key shown in this session, or null, in which case the placeholder asks for one.
 */
export function e2bSetupText(origin: string, key: string | null): string {
  return [`E2B_API_URL=${origin}/e2b`, `E2B_API_KEY=${key ?? '<your key>'}`, `E2B_SANDBOX_URL=${SANDBOX_GATEWAY}`].join('\n');
}

/** The buttons a sandbox row offers: a running sandbox pauses, a paused one resumes, and either can be ended. */
export function sandboxActions(state: SandboxState): SandboxAction[] {
  return state === 'running' ? ['pause', 'end'] : ['resume', 'end'];
}

/** A memory size as the table reads it: MB below a gibibyte, GB from there (one decimal). */
export function memoryText(mb: number): string {
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${mb} MB`;
}

/** An RFC 3339 time as a local date and time; an unreadable value shows a dash. */
export function sandboxTime(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? '—' : d.toLocaleString();
}

const ACTIONS: Record<SandboxAction, { label: string; done: string; send: (id: string) => Promise<unknown> }> = {
  pause: { label: 'Pause', done: 'Paused the sandbox', send: (id) => e2bApi.pauseSandbox(id) },
  resume: { label: 'Resume', done: 'Resumed the sandbox', send: (id) => e2bApi.resumeSandbox(id) },
  end: { label: 'End', done: 'Ended the sandbox', send: (id) => e2bApi.endSandbox(id) },
};

/** A copy button for one piece of text. The text is not stored; the click copies it and says so. */
function copyButton(text: string, label = 'Copy'): HTMLButtonElement {
  return button(label, () => void attempt(() => navigator.clipboard.writeText(text), 'Copied'), { kind: 'ghost', size: 'sm' });
}

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  let alive = true;
  let keysSeq = 0;
  let sandboxesSeq = 0;
  let keys: E2bKey[] | null = null;
  let keysError: unknown = null;
  let sandboxes: E2bSandbox[] | null = null;
  let sandboxesError: unknown = null;
  // The key made in this visit: the setup text names it, and the reveal box shows it. Neither outlives the screen.
  let madeKey: string | null = null;
  let timer: number | undefined;

  const keysSlot = h('div', null, h('p.mcp-loading', null, 'Loading the keys...'));
  const keyError = h('p.mcp-danger');
  const revealSlot = h('div.hub-reveal-slot');
  const setupSlot = h('div.stack');
  const sandboxError = h('p.mcp-danger');
  const sandboxSlot = h('div', null, h('p.mcp-loading', null, 'Loading the sandboxes...'));

  // ---- Keys -----------------------------------------------------------------------------------------------------

  const loadKeys = async (): Promise<void> => {
    const seq = ++keysSeq;
    try {
      const list = await e2bApi.keys();
      if (!alive || seq !== keysSeq) return;
      keys = list;
      keysError = null;
    } catch (err) {
      if (!alive || seq !== keysSeq) return;
      keys = null;
      keysError = err;
    }
    drawKeys();
  };

  const revokeKey = (k: E2bKey): void => {
    const close = modal(
      `Revoke ${k.label}`,
      h(
        'div.mcp-confirm',
        null,
        h('p', null, 'Clients that use this key stop working. This cannot be undone.'),
        h(
          'div.mcp-actions',
          null,
          button('Cancel', () => close()),
          button(
            'Revoke',
            () => {
              close();
              void doRevoke(k);
            },
            { kind: 'danger' },
          ),
        ),
      ),
    );
  };

  const doRevoke = async (k: E2bKey): Promise<void> => {
    replace(keyError);
    try {
      await e2bApi.revokeKey(k.id);
      if (!alive) return;
      toast(`Revoked ${k.label}`);
    } catch (err) {
      if (alive) replace(keyError, errorText(err));
    }
    if (alive) void loadKeys();
  };

  const keyRow = (k: E2bKey): HTMLElement =>
    h(
      'tr',
      null,
      h('td', { 'data-label': 'Label' }, k.label),
      h('td', { 'data-label': 'Created' }, ago(k.created_ms)),
      h('td', { 'data-label': 'Actions', class: 'mcp-actions-cell' }, button('Revoke', () => revokeKey(k), { kind: 'danger', size: 'sm', icon: 'trash' })),
    );

  const drawKeys = (): void => {
    if (keysError) {
      replace(keysSlot, empty('alert', 'Could not load the keys', errorText(keysError)));
      return;
    }
    if (!keys) return;
    replace(
      keysSlot,
      keys.length
        ? h(
            'div.mcp-table-wrap',
            null,
            h(
              'table.mcp-table',
              null,
              h('thead', null, h('tr', null, h('th', null, 'Label'), h('th', null, 'Created'), h('th', null, 'Actions'))),
              h('tbody', null, keys.map(keyRow)),
            ),
          )
        : empty('key', 'No keys', 'Make a key below for an SDK client.'),
    );
  };

  const labelInput = input({ maxlength: 80, placeholder: 'laptop-sdk', autocomplete: 'off' });

  const createKey = async (): Promise<void> => {
    const label = labelInput.value.trim();
    if (!label) {
      replace(keyError, 'Enter a label for the key, such as ci or laptop.');
      return;
    }
    replace(keyError);
    createBtn.disabled = true;
    try {
      const made: E2bKeyCreated = await e2bApi.createKey(label);
      if (!alive) return;
      labelInput.value = '';
      madeKey = made.key;
      showReveal(made);
      drawSetup();
      void loadKeys();
    } catch (err) {
      if (alive) replace(keyError, errorText(err));
    } finally {
      createBtn.disabled = false;
    }
  };

  const createBtn = button('Create key', () => void createKey(), { kind: 'primary', icon: 'plus' });

  /** The new key, once. The reveal box is the only place it is shown; it leaves with the screen. */
  const showReveal = (made: E2bKeyCreated): void => {
    replace(
      revealSlot,
      h(
        'div.hub-reveal',
        null,
        h('h3', null, `Key for ${made.label}`),
        h('p.hub-warn', null, 'Shown once. Copy it now, because it cannot be shown again.'),
        h('div.hub-token-row', null, h('code.mcp-mono.hub-token', null, made.key), copyButton(made.key, 'Copy key')),
        button('Hide', () => replace(revealSlot), { kind: 'ghost' }),
      ),
    );
  };

  // ---- Setup ----------------------------------------------------------------------------------------------------

  const drawSetup = (): void => {
    const text = e2bSetupText(location.origin, madeKey);
    replace(
      setupSlot,
      h(
        'div.hub-snippet',
        null,
        h('div.hub-snippet-head', null, h('strong', null, 'Environment'), copyButton(text)),
        h('pre.mcp-mono', null, text),
        h(
          'p.mcp-muted',
          null,
          madeKey
            ? 'The key above is in this setup. It lives only on this screen, so copy it into the client before you leave.'
            : 'Put a key in E2B_API_KEY. Make one above; it is shown once.',
        ),
        h(
          'p.mcp-muted',
          null,
          'E2B_SANDBOX_URL is the hub machine\'s own local sandbox gateway, so it is the same on every client that runs there. Use it as written.',
        ),
      ),
    );
  };

  // ---- Sandboxes ------------------------------------------------------------------------------------------------

  const loadSandboxes = async (): Promise<void> => {
    const seq = ++sandboxesSeq;
    try {
      const list = await e2bApi.sandboxes();
      if (!alive || seq !== sandboxesSeq) return;
      sandboxes = list;
      sandboxesError = null;
    } catch (err) {
      if (!alive || seq !== sandboxesSeq) return;
      sandboxes = null;
      sandboxesError = err;
    }
    drawSandboxes();
  };

  const runAction = async (s: E2bSandbox, a: SandboxAction): Promise<void> => {
    replace(sandboxError);
    try {
      await ACTIONS[a].send(s.sandboxID);
      if (!alive) return;
      toast(ACTIONS[a].done);
    } catch (err) {
      if (alive) replace(sandboxError, errorText(err));
    }
    if (alive) void loadSandboxes();
  };

  // Every sandbox action asks first, in words that say what it does to the sandbox.
  const CONFIRM: Record<SandboxAction, string> = {
    pause: 'Pauses the sandbox. It keeps its state and stops running until it is resumed.',
    resume: 'Resumes the sandbox so it runs again.',
    end: 'Ends the sandbox. Whatever it holds is lost, and it cannot be resumed.',
  };

  const confirmAction = (s: E2bSandbox, a: SandboxAction): void => {
    const close = modal(
      `${ACTIONS[a].label} this sandbox`,
      h(
        'div.mcp-confirm',
        null,
        h('p', null, CONFIRM[a], ' ', h('span.mcp-mono', null, s.sandboxID)),
        h(
          'div.mcp-actions',
          null,
          button('Cancel', () => close()),
          button(
            ACTIONS[a].label,
            () => {
              close();
              void runAction(s, a);
            },
            { kind: a === 'end' ? 'danger' : 'primary' },
          ),
        ),
      ),
    );
  };

  const actionButton = (s: E2bSandbox, a: SandboxAction): HTMLButtonElement =>
    button(ACTIONS[a].label, () => confirmAction(s, a), {
      kind: a === 'end' ? 'danger' : 'ghost',
      size: 'sm',
      icon: a === 'end' ? 'stop' : a === 'resume' ? 'play' : undefined,
    });

  const sandboxRow = (s: E2bSandbox): HTMLElement =>
    h(
      'tr',
      null,
      h('td', { 'data-label': 'Sandbox' }, h('span.mcp-mono', null, s.sandboxID)),
      h('td', { 'data-label': 'Template' }, s.templateID),
      h('td', { 'data-label': 'State' }, badge(s.state, s.state === 'running' ? 'ok' : 'muted')),
      h('td', { 'data-label': 'Started' }, sandboxTime(s.startedAt)),
      h('td', { 'data-label': 'Ends' }, sandboxTime(s.endAt)),
      h('td', { 'data-label': 'CPU' }, `${s.cpuCount} vCPU`),
      h('td', { 'data-label': 'Memory' }, memoryText(s.memoryMB)),
      h('td', { 'data-label': 'CID' }, s.cid == null ? '—' : String(s.cid)),
      h('td', { 'data-label': 'Actions', class: 'mcp-actions-cell' }, sandboxActions(s.state).map((a) => actionButton(s, a))),
    );

  const drawSandboxes = (): void => {
    if (sandboxesError) {
      replace(sandboxSlot, empty('alert', 'Could not load the sandboxes', errorText(sandboxesError)));
      return;
    }
    if (!sandboxes) return;
    replace(
      sandboxSlot,
      sandboxes.length
        ? h(
            'div.mcp-table-wrap',
            null,
            h(
              'table.mcp-table',
              null,
              h(
                'thead',
                null,
                h(
                  'tr',
                  null,
                  ['Sandbox', 'Template', 'State', 'Started', 'Ends', 'CPU', 'Memory', 'CID', 'Actions'].map((c) => h('th', null, c)),
                ),
              ),
              h('tbody', null, sandboxes.map(sandboxRow)),
            ),
          )
        : empty('harness', 'No sandboxes', 'The gateway runs none right now. A client that makes one shows it here.'),
    );
  };

  // ---- Page -----------------------------------------------------------------------------------------------------

  const keysCard = card('Keys', [
    keysSlot,
    revealSlot,
    h('div.stack', null, h('h3', null, 'New key'), field('Label', labelInput, 'What this client is, such as ci or laptop-sdk.'), h('div.row', null, createBtn), keyError),
  ]);
  const setupCard = card('Setup for SDK clients', setupSlot);
  const sandboxCard = card('Sandboxes', [sandboxError, sandboxSlot]);

  replace(root, h('div.mcp-page', null, h('div.stack', null, keysCard, setupCard, sandboxCard)));
  drawSetup();
  drawKeys();
  drawSandboxes();
  void loadKeys();
  void loadSandboxes();
  timer = window.setInterval(() => void loadSandboxes(), SANDBOX_REFRESH_MS);

  return () => {
    alive = false;
    if (timer !== undefined) clearInterval(timer);
    madeKey = null;
  };
}
