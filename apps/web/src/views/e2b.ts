// Setup > E2B API: the owner's keys for the hub's E2B gateway, the setup an SDK client needs, the sandboxes the gateway
// runs, and the templates they start from (all refreshed every 10 s while the screen is open). A key is shown once, in
// the reveal box, and is never stored: leaving the screen clears it. The pure helpers (the setup text, the actions a
// sandbox row offers, the memory text, the template builds) are exported and unit-tested (test/tests.ts).

import '../styles/proto/settings-d.css';
import {
  e2bApi,
  type BuildStatus,
  type E2bBuild,
  type E2bKey,
  type E2bKeyCreated,
  type E2bSandbox,
  type E2bTemplate,
} from '../api';
export type { BuildStatus, E2bBuild, E2bTemplate } from '../api';
import { attempt, badge, card, field, input, toast, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { errorText } from './vault-common';
import { ago, btn, dialog, empty, head, ptable, type PCol } from './workstations-ui';

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

/** The template's newest build (the hub lists builds newest first), or null when it has none. */
export function latestBuild(t: E2bTemplate): E2bBuild | null {
  return t.builds[0] ?? null;
}

/** The tags of all ready builds, each once, in the order the builds list them. */
export function templateTags(t: E2bTemplate): string[] {
  return [...new Set(t.builds.filter((b) => b.status === 'ready').flatMap((b) => b.tags))];
}

/** The newest build's failure message when the newest build failed; null when it did not (or there is none). */
export function templateFailure(t: E2bTemplate): string | null {
  const b = latestBuild(t);
  return b?.status === 'error' ? (b.reason?.message ?? 'The build failed.') : null;
}

const ACTIONS: Record<SandboxAction, { label: string; done: string; send: (id: string) => Promise<unknown> }> = {
  pause: { label: 'Pause', done: 'Paused the sandbox', send: (id) => e2bApi.pauseSandbox(id) },
  resume: { label: 'Resume', done: 'Resumed the sandbox', send: (id) => e2bApi.resumeSandbox(id) },
  end: { label: 'End', done: 'Ended the sandbox', send: (id) => e2bApi.endSandbox(id) },
};

const BUILD_TONE: Record<BuildStatus, Tone> = { waiting: 'muted', building: 'info', ready: 'ok', error: 'danger' };



/** A copy button for one piece of text. The text is not stored; the click copies it and says so. */
function copyButton(text: string, label = 'Copy'): HTMLButtonElement {
  return btn(label, () => void attempt(() => navigator.clipboard.writeText(text), 'Copied'), { kind: 'ghost', sm: true });
}

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  let alive = true;
  let keysSeq = 0;
  let sandboxesSeq = 0;
  let templatesSeq = 0;
  let keys: E2bKey[] | null = null;
  let keysError: unknown = null;
  let sandboxes: E2bSandbox[] | null = null;
  let sandboxesError: unknown = null;
  let templates: E2bTemplate[] | null = null;
  let templatesError: unknown = null;
  // The key made in this visit: the setup text names it, and the reveal box shows it. Neither outlives the screen.
  let madeKey: string | null = null;
  let madeLabel = '';
  let timer: number | undefined;

  const keysSlot = h('div', null, h('p.muted', null, 'Loading the keys...'));
  const keyError = h('div.e2b-err');
  const revealSlot = h('div');
  const setupSlot = h('div');
  const sandboxError = h('div.e2b-err');
  const sandboxSlot = h('div', null, h('p.muted', null, 'Loading the sandboxes...'));
  const templateError = h('div.e2b-err');
  const templateSlot = h('div', null, h('p.muted', null, 'Loading the templates...'));

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
    const close = dialog(`Revoke ${k.label}`, h('p', null, 'Clients that use this key stop working. This cannot be undone.'), [
      btn('Cancel', () => close(), { kind: 'ghost' }),
      btn(
        'Revoke',
        () => {
          close();
          void doRevoke(k);
        },
        { kind: 'danger' },
      ),
    ]);
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

  const keyColumns: PCol<E2bKey>[] = [
    { label: 'Label', cell: (k) => k.label },
    { label: 'Created', cell: (k) => ago(k.created_ms) },
    { label: '', cls: 'num', cell: (k) => btn('Revoke', () => revokeKey(k), { kind: 'danger', sm: true, icon: 'trash' }) },
  ];

  const drawKeys = (): void => {
    if (keysError) {
      replace(keysSlot, empty('alert', 'Could not load the keys', errorText(keysError)));
      return;
    }
    if (!keys) return;
    replace(keysSlot, keys.length ? ptable(keyColumns, keys) : empty('key', 'No keys', 'Make a key below for an SDK client.'));
  };

  const labelInput = input({ maxlength: 80, placeholder: 'laptop-sdk', autocomplete: 'off', id: 'e2b-label' });

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
      madeLabel = made.label;
      showReveal();
      drawSetup();
      void loadKeys();
    } catch (err) {
      if (alive) replace(keyError, errorText(err));
    } finally {
      createBtn.disabled = false;
    }
  };

  const createBtn = btn('Create key', () => void createKey(), { kind: 'primary', icon: 'plus' });

  /** The new key, once. The reveal box is the only place it is shown; it leaves with the screen. */
  const showReveal = (): void => {
    if (!madeKey) return replace(revealSlot);
    replace(
      revealSlot,
      h(
        'div.e2b-reveal',
        null,
        h('b', null, `Key for ${madeLabel}`),
        h('div.e2b-env', null, madeKey),
        h('div.ws-cluster', null, copyButton(madeKey, 'Copy key'), btn('Hide', () => hideReveal(), { kind: 'ghost' })),
      ),
    );
  };

  const hideReveal = (): void => {
    madeKey = null;
    replace(revealSlot);
    drawSetup();
  };

  // ---- Setup ----------------------------------------------------------------------------------------------------

  const drawSetup = (): void => {
    const text = e2bSetupText(location.origin, madeKey);
    replace(
      setupSlot,
      h('pre.e2b-env', null, text),
      h('p.hint', null, "E2B_SANDBOX_URL is the hub machine's own local sandbox gateway, so it is the same on every client that runs there. Use it as written."),
      h('div.ws-cluster', null, copyButton(text)),
      h(
        'p.muted',
        null,
        madeKey
          ? 'The key above is in this setup. It lives only on this screen, so copy it into the client before you leave.'
          : 'Put a key in E2B_API_KEY. Make one above; it is shown once.',
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
    const close = dialog(
      `${ACTIONS[a].label} this sandbox`,
      [h('p', null, CONFIRM[a]), h('p.ws-mono', null, s.sandboxID)],
      [
        btn('Cancel', () => close(), { kind: 'ghost' }),
        btn(
          ACTIONS[a].label,
          () => {
            close();
            void runAction(s, a);
          },
          { kind: a === 'end' ? 'danger' : 'primary' },
        ),
      ],
    );
  };

  const actionButton = (s: E2bSandbox, a: SandboxAction): HTMLButtonElement =>
    btn(ACTIONS[a].label, () => confirmAction(s, a), {
      kind: a === 'end' ? 'danger' : 'ghost',
      sm: true,
      icon: a === 'resume' ? 'play' : a === 'end' ? 'stop' : undefined,
    });

  const sandboxColumns: PCol<E2bSandbox>[] = [
    { label: 'Sandbox', cell: (s) => h('span.ws-mono', null, s.sandboxID) },
    { label: 'Template', cell: (s) => s.templateID },
    { label: 'State', cell: (s) => badge(s.state, s.state === 'running' ? 'ok' : 'muted', true) },
    { label: 'Started', cell: (s) => sandboxTime(s.startedAt) },
    { label: 'Ends', cell: (s) => sandboxTime(s.endAt) },
    { label: 'CPU', cell: (s) => `${s.cpuCount} vCPU` },
    { label: 'Memory', cell: (s) => memoryText(s.memoryMB) },
    { label: 'CID', cell: (s) => (s.cid == null ? '—' : String(s.cid)) },
    { label: 'Actions', cls: 'num', cell: (s) => sandboxActions(s.state).map((a) => actionButton(s, a)) },
  ];

  const drawSandboxes = (): void => {
    if (sandboxesError) {
      replace(sandboxSlot, empty('alert', 'Could not load the sandboxes', errorText(sandboxesError)));
      return;
    }
    if (!sandboxes) return;
    replace(
      sandboxSlot,
      sandboxes.length
        ? ptable(sandboxColumns, sandboxes)
        : empty('harness', 'No sandboxes', 'The gateway runs none right now. A client that makes one shows it here.'),
    );
  };

  // ---- Templates ------------------------------------------------------------------------------------------------

  const loadTemplates = async (): Promise<void> => {
    const seq = ++templatesSeq;
    try {
      const list = await e2bApi.templates();
      if (!alive || seq !== templatesSeq) return;
      templates = list;
      templatesError = null;
    } catch (err) {
      if (!alive || seq !== templatesSeq) return;
      templates = null;
      templatesError = err;
    }
    drawTemplates();
  };

  const doDeleteTemplate = async (t: E2bTemplate): Promise<void> => {
    replace(templateError);
    try {
      await e2bApi.deleteTemplate(t.templateID);
      if (!alive) return;
      toast(`Deleted ${t.name}`);
    } catch (err) {
      if (alive) replace(templateError, errorText(err));
    }
    if (alive) void loadTemplates();
  };

  // Deleting a template asks first. A build that runs, or one that starts from it, makes the hub refuse (409).
  const confirmDeleteTemplate = (t: E2bTemplate): void => {
    const close = dialog(
      `Delete ${t.name}`,
      [
        h('p', null, 'Deletes the template and all its builds. This cannot be undone. A build that is running, or one that starts from it, blocks the delete until it finishes.'),
        h('p.ws-mono', null, t.templateID),
      ],
      [
        btn('Cancel', () => close(), { kind: 'ghost' }),
        btn(
          'Delete',
          () => {
            close();
            void doDeleteTemplate(t);
          },
          { kind: 'danger' },
        ),
      ],
    );
  };

  const templateColumns: PCol<E2bTemplate>[] = [
    { label: 'Template', cell: (t) => [h('b', null, t.name), ' ', h('span.ws-mono.muted', null, t.templateID.slice(0, 12))] },
    {
      label: 'Build',
      cell: (t) => {
        const latest = latestBuild(t);
        return latest ? badge(latest.status, BUILD_TONE[latest.status], true) : '—';
      },
    },
    { label: 'Tags', cell: (t) => templateTags(t).join(', ') || '—' },
    {
      label: 'Size',
      cell: (t) => {
        const latest = latestBuild(t);
        return latest ? `${latest.cpuCount} vCPU, ${memoryText(latest.memoryMB)}` : '—';
      },
    },
    { label: 'Updated', cell: (t) => sandboxTime(t.updatedAt) },
    { label: 'Failure', cell: (t) => templateFailure(t) ?? '—' },
    { label: '', cls: 'num', cell: (t) => btn('Delete', () => confirmDeleteTemplate(t), { kind: 'danger', sm: true, icon: 'trash' }) },
  ];

  const drawTemplates = (): void => {
    if (templatesError) {
      replace(templateSlot, empty('alert', 'Could not load the templates', errorText(templatesError)));
      return;
    }
    if (!templates) return;
    replace(
      templateSlot,
      templates.length
        ? ptable(templateColumns, templates)
        : empty('harness', 'No templates', "Templates are built with the E2B SDK's Template.build."),
    );
  };

  // ---- Page -----------------------------------------------------------------------------------------------------

  root.append(
    head('E2B API'),
    h(
      'div.ws-stack',
      null,
      card('Keys', [
        keysSlot,
        revealSlot,
        h('div.e2b-form', null, h('b', null, 'New key'), field('Label', labelInput, 'What this client is, such as ci or laptop-sdk.'), createBtn),
        keyError,
      ]),
      card('Setup for SDK clients', setupSlot),
      card('Sandboxes', [sandboxError, sandboxSlot], { flush: true }),
      card('Templates', [templateError, templateSlot], { flush: true }),
    ),
  );
  drawSetup();
  drawKeys();
  drawSandboxes();
  drawTemplates();
  void loadKeys();
  void loadSandboxes();
  void loadTemplates();
  timer = window.setInterval(() => {
    void loadSandboxes();
    void loadTemplates();
  }, SANDBOX_REFRESH_MS);

  return () => {
    alive = false;
    if (timer !== undefined) clearInterval(timer);
    madeKey = null;
  };
}
