// The Nodes screen: the machines that make up the plaza, each stopped and
// resumed on demand (its agents move to the other machines, and back when it
// returns), a rebalance that spreads the agents over them, the join codes that
// bring new ones in (each token shown once), this hub's own identity with the
// owner-token rotation, and this hub joined to another network as a node.
// Refreshes every few seconds.

import { api, type Agent, type HubIdentity, type JoinCode, type MachineNode, type NodeLink, type NodeView, type Rebalance } from '../api';
import { ago, attempt, badge, button, card, empty, field, input, modal, select, table, toast, type Column, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { isLive, KIND_LABEL, machineDetail, machineList, newRunState, normalizeMachines, specOf, type ListRow } from './machine-list';
import type { Cleanup } from './index';

const REFRESH_MS = 5000;
const HUB_NODE = 'n0';

const JOIN_TONE: Record<JoinCode['state'], Tone> = { active: 'ok', used: 'info', expired: 'muted', revoked: 'danger' };
const JOIN_LABEL: Record<JoinCode['state'], string> = { active: 'Active', used: 'Used', expired: 'Expired', revoked: 'Revoked' };

/** The state a node shows, most urgent first. A stopped node is stopped whether its machine is on or off. */
export function nodeState(n: NodeView): { label: string; tone: Tone } {
  if (n.revoked) return { label: 'revoked', tone: 'danger' };
  if (n.state === 'stopping') return { label: `stopping, ${n.left} ${n.left === 1 ? 'agent' : 'agents'} to move`, tone: 'warn' };
  if (n.state === 'stopped' || n.draining) return { label: 'stopped', tone: 'info' };
  return n.online ? { label: 'online', tone: 'ok' } : { label: 'offline', tone: 'muted' };
}

/** Whether the owner took the node out of service (stopping or stopped). */
export function isStopped(n: NodeView): boolean {
  return n.draining || n.state === 'stopping' || n.state === 'stopped';
}

/** One line per agent that cannot leave a stopping node yet: its name (or CID) and why. */
export function waitingLines(n: NodeView, nameOf: (cid: number) => string | undefined): string[] {
  return (n.waiting ?? []).map((w) => `${nameOf(w.cid) ?? `Agent ${w.cid}`}: ${w.reason}`);
}

/** What a rebalance did, for its toast. */
export function rebalanceText(r: Rebalance): string {
  const n = r.moves.length;
  if (!n) return 'The agents are already spread over the machines.';
  return `Moving ${n} ${n === 1 ? 'agent' : 'agents'} to spread the load.`;
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

/** The CID range this node's agents hold in the network it joined, first to last. */
function linkCids(c: NodeLink['cids']): string {
  return c ? c.replace('-', '–') : '—';
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
  const list = machineList();
  const codesBody = h('div');
  const hubBody = h('div');
  const linkBody = h('div.node-link');
  root.append(
    h(
      'div.view-head.node-head',
      null,
      h('h1', null, 'Nodes'),
      h(
        'div.node-buttons',
        null,
        button('Rebalance', () => void rebalance(), { title: 'Spread the idle agents over the machines in service, by memory' }),
        button('Add a node', () => openAdd(), { kind: 'primary' }),
      ),
    ),
    card('Machines in this plaza', list.el, { flush: true }),
    card('Join another network', linkBody),
    card('Join codes', codesBody, { flush: true }),
    card('This hub', hubBody),
  );

  let alive = true;
  let ready = false;
  // null: the last request failed.
  let nodes: NodeView[] | null = null;
  let codes: JoinCode[] | null = null;
  let hub: HubIdentity | null = null;
  let link: NodeLink | null = null;
  // Each node's machine, joined by id (the hub is n0), and the agents' names by CID for the machine detail.
  let machines = new Map<string, MachineNode>();
  let agentsByCid = new Map<number, Agent>();
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
    const [n, c, id, ln, ms, ag] = await Promise.all([
      api.nodes().catch(() => null),
      api.joinCodes().catch(() => null),
      api.hubIdentity().catch(() => null),
      api.nodeLink().catch(() => null),
      api.machines().catch(() => null),
      api.agents().catch((): Agent[] => []),
    ]);
    if (!alive) return;
    nodes = n;
    codes = c;
    hub = id;
    link = ln;
    machines = new Map(normalizeMachines(ms).map((m): [string, MachineNode] => [m.id, m]));
    agentsByCid = new Map((ag ?? []).map((a): [number, Agent] => [a.cid, a]));
    ready = true;
    renderNodes();
    renderCodes();
    renderHub();
    renderLink();
  };

  const stop = async (n: NodeView, now: boolean): Promise<void> => {
    await attempt(() => api.stopNode(n.id, now), now ? `${n.name} is stopping: its agents move now` : `${n.name} is stopping: its agents move as their turns end`);
    void load();
  };

  const resume = async (n: NodeView): Promise<void> => {
    await attempt(() => api.resumeNode(n.id), `${n.name} is back in service: agents move onto it once it reports`);
    void load();
  };

  const rebalance = async (): Promise<void> => {
    const r = await attempt(() => api.rebalance());
    if (r) toast(rebalanceText(r));
    void load();
  };

  const confirmStop = (n: NodeView): void => {
    const hub = n.id === HUB_NODE;
    const close = show(
      `Stop ${n.name}?`,
      h(
        'div.stack',
        null,
        h(
          'p',
          null,
          n.agents
            ? `Its ${n.agents === 1 ? 'agent moves' : `${n.agents} agents move`} to the other machines in service, one at a time, and no new agent starts on it.`
            : 'No new agent starts on it.',
        ),
        h('p', null, hub ? 'This is the hub’s own machine: the hub keeps running, only its agents leave.' : 'Once it is stopped, its machine can be turned off. Resume brings it back, and agents move onto it again.'),
        h('p.faint', null, 'Stop now interrupts the turns in progress; each one resumes on its agent’s new machine. Otherwise an agent that is working moves when its turn ends.'),
        h(
          'div.node-buttons',
          null,
          button('Cancel', () => close()),
          button('Stop after their turns', () => {
            close();
            void stop(n, false);
          }),
          button(
            'Stop now',
            () => {
              close();
              void stop(n, true);
            },
            { kind: 'danger' },
          ),
        ),
      ),
    );
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
    if (n.revoked) return null;
    const hub = n.id === HUB_NODE;
    return h(
      'div.node-actions',
      null,
      hub ? h('span.faint', null, 'this hub') : null,
      isStopped(n) ? button('Resume', () => void resume(n), { size: 'sm' }) : button('Stop', () => confirmStop(n), { size: 'sm' }),
      n.state === 'stopping' ? button('Move now', () => void stop(n, true), { size: 'sm', title: 'Interrupt the turns in progress: each resumes on its agent’s new machine' }) : null,
      hub ? null : button('Revoke', () => confirmRevoke(n), { kind: 'danger', size: 'sm' }),
    );
  };

  // The run kills in a node's detail: then the page reloads.
  const killRun = async (id: string): Promise<void> => {
    await attempt(() => api.killRun(id), `Killed run ${id}`);
    void load();
  };
  const rs = newRunState((id) => void killRun(id));

  // A node's detail: its machine's usage and runs when it is joined to one, then its facts and actions.
  const nodeDetail = (n: NodeView, m: MachineNode | undefined): HTMLElement => {
    const pairs: [string, string][] = [
      ['Address', n.address || '—'],
      ['CIDs', cids(n)],
      ['Last seen', ago(n.last_seen_ms)],
      ['Agents', String(n.agents)],
      ['Host', n.machine?.host ?? '—'],
      ['Version', n.machine?.version ?? '—'],
    ];
    const waits = waitingLines(n, (cid) => agentsByCid.get(cid)?.name);
    return h(
      'div.stack',
      null,
      m ? machineDetail(m, agentsByCid, rs) : null,
      h('dl.node-facts', null, pairs.flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)])),
      waits.length ? h('div.stack', null, h('strong', null, 'Waiting to move'), h('ul.node-waiting', null, waits.map((line) => h('li', null, line)))) : null,
      h('div.node-buttons', null, nodeActions(n)),
    );
  };

  // A node's row: its machine's picture when it is joined to one, the line art otherwise.
  const rowOf = (n: NodeView): ListRow => {
    const m = machines.get(n.id);
    const s = nodeState(n);
    return {
      key: n.id,
      machineId: m?.id ?? null,
      name: n.name,
      subtitle: m ? [KIND_LABEL[m.kind], m.model].filter(Boolean).join(' · ') : n.id,
      status: s,
      facts: [
        ['Address', n.address || '—'],
        ['Agents', String(n.agents)],
        ['CIDs', cids(n)],
        ['Machine', machineText(n.machine)],
        ['Last seen', ago(n.last_seen_ms)],
      ],
      bars: [],
      spec: m ? specOf(m) : null,
      active: m ? isLive(m.runs ?? []) : n.online,
      actions: nodeActions(n),
      detail: () => nodeDetail(n, m),
    };
  };

  const renderNodes = (): void => {
    if (!ready) return;
    list.setRows(
      nodes ? nodes.map(rowOf) : [],
      nodes === null
        ? empty('alert', 'Could not load the machines.')
        : nodes.length
          ? null
          : empty('link', 'No machines yet', 'Add a node here to bring a machine into this network, or join this hub to another network below.'),
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

  // ---- Join another network: this hub lends its machine to that network ----

  const linkLede = h(
    'p.node-link-lede',
    null,
    "Make this hub's machine a node of another network: it lends its machine to that network's agents. Its own plaza keeps running.",
  );
  const joinCode = input({ placeholder: 'Paste the join code', maxlength: 200, autocomplete: 'off', spellcheck: 'false', 'aria-label': 'Join code', class: 'node-code' });
  const joinError = h('p.error-text');
  const joinButton = button('Join', () => void join(), { kind: 'primary' });
  const joinPane = h(
    'div.stack.node-link-join',
    null,
    field('Join code', joinCode, "Get a code from the other network's Nodes screen (Add a node)."),
    h('div.node-buttons', null, joinButton),
    joinError,
  );
  const joinedPane = h('div.stack');
  linkBody.append(linkLede, joinPane, joinedPane);

  const linkFacts = (l: NodeLink): HTMLElement =>
    h(
      'div.stack',
      null,
      h(
        'dl.node-facts',
        null,
        h('dt', null, 'Hub'),
        h('dd', null, l.hub ?? '—'),
        h('dt', null, 'This node'),
        h('dd', null, l.node_id ?? '—'),
        h('dt', null, 'CIDs'),
        h('dd', null, linkCids(l.cids)),
        h('dt', null, 'Status'),
        h('dd', null, badge(l.running ? 'running' : 'stopped', l.running ? 'ok' : 'muted', true), l.running && l.pid ? h('span.faint', null, ` pid ${l.pid}`) : null),
      ),
      h('div.node-buttons', null, button('Leave', () => confirmLeave(), { kind: 'danger' })),
    );

  const renderLink = (): void => {
    if (!ready) return;
    const l = link;
    if (!l) {
      joinPane.hidden = true;
      joinedPane.hidden = false;
      replace(joinedPane, empty('alert', 'Could not load the network link.'));
      return;
    }
    joinPane.hidden = l.joined;
    joinedPane.hidden = !l.joined;
    if (l.joined) replace(joinedPane, linkFacts(l));
  };

  const join = async (): Promise<void> => {
    const code = joinCode.value.trim();
    joinError.textContent = '';
    if (!code) {
      joinError.textContent = 'Paste the join code first.';
      return;
    }
    joinButton.disabled = true;
    try {
      link = await api.joinNodeLink(code);
      joinCode.value = '';
      toast('Joined the network');
      renderLink();
    } catch (err) {
      joinError.textContent = err instanceof Error ? err.message : String(err);
      void load();
    } finally {
      joinButton.disabled = false;
    }
  };

  const leave = async (forget: boolean): Promise<void> => {
    const status = await attempt(() => api.leaveNodeLink(forget), 'Left the network');
    if (!status) return;
    link = status;
    renderLink();
  };

  const confirmLeave = (): void => {
    const forget = h('input', { type: 'checkbox' }) as HTMLInputElement;
    const close = show(
      'Leave this network?',
      h(
        'div.stack',
        null,
        h('p', null, 'This node stops lending its machine to that network, and its agents there go offline.'),
        h('label.node-forget', null, forget, h('span', null, 'Forget this network: remove the saved hub, so joining again needs a new code.')),
        h(
          'div.node-buttons',
          null,
          button('Cancel', () => close()),
          button(
            'Leave',
            () => {
              close();
              void leave(forget.checked);
            },
            { kind: 'danger' },
          ),
        ),
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
    list.destroy();
  };
}
