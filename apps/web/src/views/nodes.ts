// The Nodes screen: the machines that make up the plaza, the join codes that
// bring new ones in (each token shown once), and this hub's own identity with
// the owner-token rotation. Refreshes every few seconds.

import { api, type HubIdentity, type JoinCode, type NodeView } from '../api';
import { ago, attempt, badge, button, card, empty, field, input, modal, select, table, toast, type Column, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

const REFRESH_MS = 5000;
const HUB_NODE = 'n0';

const JOIN_TONE: Record<JoinCode['state'], Tone> = { active: 'ok', used: 'info', expired: 'muted', revoked: 'danger' };
const JOIN_LABEL: Record<JoinCode['state'], string> = { active: 'Active', used: 'Used', expired: 'Expired', revoked: 'Revoked' };

/** The state a node shows, most urgent first. */
function nodeState(n: NodeView): { label: string; tone: Tone } {
  if (n.revoked) return { label: 'revoked', tone: 'danger' };
  if (n.draining) return { label: 'draining', tone: 'warn' };
  return n.online ? { label: 'online', tone: 'ok' } : { label: 'offline', tone: 'muted' };
}

/** mm:ss left until a deadline, 00:00 once it has passed. */
function countdown(leftMs: number): string {
  const s = Math.max(0, Math.floor(leftMs / 1000));
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
}

/** The CID range a node's agents hold, first to last. */
function cids(n: NodeView): string {
  return n.cid_count > 0 ? `${n.cid_base}–${n.cid_base + n.cid_count - 1}` : '—';
}

/** cpus · memory GiB · host, as the node reported them. */
function machineText(m: NodeView['machine']): string {
  if (!m) return '—';
  const memory = m.memory_mib ? `${(m.memory_mib / 1024).toFixed(1)} GiB` : '?';
  return `${m.cpus ?? '?'} cpus · ${memory} · ${m.host ?? '?'}`;
}

/** A value to copy: a mono box with a Copy button. */
function copyBox(value: string): HTMLElement {
  return h(
    'div.node-token',
    null,
    h('code', null, value),
    button('Copy', () => void attempt(() => navigator.clipboard.writeText(value), 'Copied'), { kind: 'ghost', size: 'sm' }),
  );
}

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  const nodesBody = h('div');
  const codesBody = h('div');
  const hubBody = h('div');
  root.append(
    h('div.view-head.node-head', null, h('h1', null, 'Nodes'), button('Add a node', () => openAdd(), { kind: 'primary' })),
    card('Machines in this plaza', nodesBody, { flush: true }),
    card('Join codes', codesBody, { flush: true }),
    card('This hub', hubBody),
  );

  let alive = true;
  let ready = false;
  // null: the last request failed.
  let nodes: NodeView[] | null = null;
  let codes: JoinCode[] | null = null;
  let hub: HubIdentity | null = null;
  // The countdown in the open dialog, and the dialog's closer.
  let tick: number | undefined;
  let closeOpen: (() => void) | null = null;

  const show = (title: string, body: Node): (() => void) => {
    window.clearInterval(tick);
    const close = modal(title, body, () => {
      window.clearInterval(tick);
      closeOpen = null;
    });
    closeOpen = close;
    return close;
  };

  // Counts a join code's expiry down in `el` until the dialog closes.
  const countdownTo = (el: HTMLElement, expiresMs: number): void => {
    const paint = (): void => {
      el.textContent = countdown(expiresMs - Date.now());
    };
    paint();
    window.clearInterval(tick);
    tick = window.setInterval(paint, 1000);
  };

  const load = async (): Promise<void> => {
    const [n, c, id] = await Promise.all([
      api.nodes().catch(() => null),
      api.joinCodes().catch(() => null),
      api.hubIdentity().catch(() => null),
    ]);
    if (!alive) return;
    nodes = n;
    codes = c;
    hub = id;
    ready = true;
    renderNodes();
    renderCodes();
    renderHub();
  };

  const drain = async (n: NodeView): Promise<void> => {
    await attempt(() => api.drainNode(n.id, !n.draining), n.draining ? `${n.name} is back in service` : `${n.name} is draining`);
    void load();
  };

  const revoke = async (n: NodeView): Promise<void> => {
    await attempt(() => api.revokeNode(n.id), `${n.name} revoked`);
    void load();
  };

  const revokeCode = async (c: JoinCode): Promise<void> => {
    await attempt(() => api.revokeJoinCode(c.id), 'Join code revoked');
    void load();
  };

  const confirmRevoke = (n: NodeView): void => {
    const close = show(
      `Revoke ${n.name}?`,
      h(
        'div.stack',
        null,
        h('p', null, 'Its signatures stop working at once and its agents go offline.'),
        h(
          'div.node-buttons',
          null,
          button('Cancel', () => close()),
          button(
            'Revoke',
            () => {
              close();
              void revoke(n);
            },
            { kind: 'danger' },
          ),
        ),
      ),
    );
  };

  const nodeActions = (n: NodeView): HTMLElement | null => {
    if (n.id === HUB_NODE) return h('span.faint', null, 'this hub');
    if (n.revoked) return null;
    return h(
      'div.node-actions',
      null,
      button(n.draining ? 'Resume' : 'Drain', () => void drain(n), { size: 'sm' }),
      button('Revoke', () => confirmRevoke(n), { kind: 'danger', size: 'sm' }),
    );
  };

  const renderNodes = (): void => {
    if (!ready) return;
    const columns: Column<NodeView>[] = [
      { label: 'Name', cell: (n) => h('div.node-name', null, h('strong', null, n.name), h('span.faint', null, n.id)) },
      {
        label: 'State',
        cell: (n) => {
          const s = nodeState(n);
          return badge(s.label, s.tone, true);
        },
      },
      { label: 'Address', cell: (n) => n.address || '—' },
      { label: 'Agents', num: true, cell: (n) => n.agents },
      { label: 'CIDs', cell: (n) => cids(n) },
      { label: 'Machine', cell: (n) => machineText(n.machine) },
      { label: 'Last seen', cell: (n) => ago(n.last_seen_ms) },
      { label: '', cell: (n) => nodeActions(n) },
    ];
    replace(
      nodesBody,
      nodes === null
        ? empty('alert', 'Could not load the machines.')
        : nodes.length
          ? table(columns, nodes, undefined, (n) => n.id)
          : empty('link', 'No machines yet.'),
    );
  };

  const renderCodes = (): void => {
    if (!ready) return;
    const columns: Column<JoinCode>[] = [
      { label: 'Label', cell: (c) => h('strong', null, c.label) },
      { label: 'ID', cell: (c) => h('span.faint', null, c.id) },
      { label: 'Created', cell: (c) => ago(c.created_ms) },
      { label: 'Expires', cell: (c) => (c.state === 'active' ? `in ${countdown(c.expires_ms - Date.now())}` : ago(c.expires_ms)) },
      {
        label: 'State',
        cell: (c) =>
          h('span', null, badge(JOIN_LABEL[c.state], JOIN_TONE[c.state], true), c.used_by ? h('span.faint', null, ` by ${c.used_by}`) : null),
      },
      { label: '', cell: (c) => (c.state === 'active' ? button('Revoke', () => void revokeCode(c), { kind: 'danger', size: 'sm' }) : null) },
    ];
    replace(
      codesBody,
      codes === null
        ? empty('alert', 'Could not load the join codes.')
        : codes.length
          ? table(
              columns,
              [...codes].sort((a, b) => b.created_ms - a.created_ms),
              undefined,
              (c) => c.id,
            )
          : empty('link', 'No join codes yet', 'Codes made with "Add a node" are listed here.'),
    );
  };

  const renderHub = (): void => {
    if (!ready) return;
    if (!hub) return replace(hubBody, empty('alert', 'Could not load this hub.'));
    const id: HubIdentity = hub;
    replace(
      hubBody,
      h(
        'dl.node-facts',
        null,
        h('dt', null, 'Name'),
        h('dd', null, id.name),
        h('dt', null, 'URL'),
        h('dd', null, id.url),
        h('dt', null, 'Version'),
        h('dd', null, id.version),
        h('dt', null, 'TLS fingerprint'),
        h('dd', null, id.fingerprint ? h('code.node-fp', null, id.fingerprint) : 'no TLS (loopback only)'),
      ),
      h('div.node-buttons', null, button('Rotate owner token', () => confirmRotate(), { kind: 'danger' })),
    );
  };

  const rotate = async (slot: HTMLElement): Promise<void> => {
    const made = await attempt(() => api.rotateOwnerToken());
    if (!made) return;
    replace(
      slot,
      h('p', null, 'The new owner token, shown once:'),
      copyBox(made.token),
      h('p.faint', null, 'Every signed-in browser must sign in again.'),
    );
    void load();
  };

  const confirmRotate = (): void => {
    const slot = h('div.stack', null);
    const close = show('Rotate the owner token?', slot);
    replace(
      slot,
      h('p', null, 'The current owner token stops working at once.'),
      h(
        'div.node-buttons',
        null,
        button('Cancel', () => close()),
        button('Rotate', () => void rotate(slot), { kind: 'danger' }),
      ),
    );
  };

  const openAdd = (): void => {
    const label = input({ placeholder: 'studio-pc', maxlength: 60, 'aria-label': 'Label' });
    const minutes = select(
      [
        { value: '15', label: '15 minutes' },
        { value: '30', label: '30 minutes' },
        { value: '60', label: '60 minutes' },
      ],
      '30',
    );
    const pane = h('div.stack', null);

    // The token, shown once, with the command to run on the new machine.
    const issued = (token: string, expiresMs: number): HTMLElement => {
      const left = h('span.node-left');
      countdownTo(left, expiresMs);
      return h(
        'div.stack',
        null,
        h('p.node-warn', null, 'Shown once. Single use. Anyone with it can join until it expires.'),
        copyBox(token),
        h('p.faint', null, 'Expires in ', left),
        h('p', null, 'On the new machine, run:'),
        copyBox(`apz-link join ${token}`),
        h('p.faint', null, "or paste it into the desktop app's Join a hub."),
      );
    };

    const create = async (): Promise<void> => {
      const name = label.value.trim();
      if (!name) return toast('Give the machine a label first.', 'error');
      const made = await attempt(() => api.issueJoinCode(name, Number(minutes.value)));
      if (!made) return;
      replace(pane, issued(made.token, made.expires_ms));
      void load();
    };

    replace(
      pane,
      field('Label', label, 'What the machine is called here.'),
      field('Valid for', minutes, 'How long the code can be used to join.'),
      h('div.node-buttons', null, button('Create join code', () => void create(), { kind: 'primary' })),
    );
    show('Add a node', pane);
  };

  void load();
  const refresh = window.setInterval(() => void load(), REFRESH_MS);

  return () => {
    alive = false;
    window.clearInterval(refresh);
    window.clearInterval(tick);
    closeOpen?.();
  };
}
