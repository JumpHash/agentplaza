// The Nodes screen: the machines that make up the plaza, each stopped and resumed on demand (its agents move
// to the other machines, and back when it returns), a rebalance that spreads the agents over them, the join
// codes that bring new ones in (each token shown once), this hub's own identity with the owner-token rotation,
// and this hub joined to another network as a node. Refreshes every few seconds.
// The markup is the prototype's (settings-c.css: nd-* and mt-*); the data and every action are the hub's.

import { agentLabel, agentName } from '../roles';
import '../styles/proto/settings-c.css';
import { api, type Agent, type HubIdentity, type JoinCode, type MachineNode, type NodeLink, type NodeView, type Rebalance } from '../api';
import { on, RESYNC } from '../store';
import { attempt, badge, button, card, empty, field, input, select, toast, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { KIND_LABEL, normalizeMachines, sentence, uptime } from './machine-list';
import { agoText, dialog, kv, pageHead, tableWrap } from './vault-ui';
import type { Cleanup } from './index';

const REFRESH_MS = 5000;
const HUB_NODE = 'n0';
const LIVE_RE = /run|ready|up|active/i;

const JOIN_CODE: Record<JoinCode['state'], [string, Tone]> = {
  active: ['Active', 'ok'],
  used: ['Used', 'info'],
  expired: ['Expired', 'muted'],
  revoked: ['Revoked', 'danger'],
};

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
  return (n.waiting ?? []).map((w) => `${nameOf(w.cid) ?? agentLabel(w.cid)}: ${w.reason}`);
}

/** What a rebalance did, for its toast. */
export function rebalanceText(r: Rebalance): string {
  const n = r.moves.length;
  if (!n) return 'The agents are already spread over the machines.';
  return `Moving ${n} ${n === 1 ? 'agent' : 'agents'} to spread the load.`;
}

/** The CID range a node's agents hold, first to last. */
function cids(n: NodeView): string {
  return `${n.cid_base}–${n.cid_base + n.cid_count - 1}`;
}

/** The CID range this node's agents hold in the network it joined, first to last. */
function linkCids(c: NodeLink['cids']): string {
  return c ? c.replace('-', '–') : '—';
}

const gib = (mib: number | undefined): string => (Number.isFinite(mib) ? ((mib as number) / 1024).toFixed(1) : '?');
const fx1 = (n: number): string => (Number.isFinite(n) ? n.toFixed(1) : '–');
const clamp = (p: number): number => Math.min(100, Math.max(0, p));

/** mm:ss from a span of milliseconds. */
function mmss(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1e3));
  return `${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
}

/** A run or VM state's tone: live words are ok, starting ones info, failures danger. */
function toneOf(s: string): Tone {
  if (LIVE_RE.test(s)) return 'ok';
  if (/start|boot|pend|creat|launch/i.test(s)) return 'info';
  if (/fail|err|crash|kill|dead/i.test(s)) return 'danger';
  return 'muted';
}

/** A chunk of markup from a fixed template (machine art, sparklines): a parsed element, nothing run. */
function svgOf(markup: string): Element {
  const t = document.createElement('template');
  t.innerHTML = markup;
  return t.content.firstElementChild ?? document.createElement('span');
}

/** The machine's line art: a chassis sized by kind, LEDs lit while a VM runs, a vent line. */
function artSvg(kind: string, active: boolean): string {
  const [w, hg] = kind === 'edge' ? [44, 44] : kind === 'server' ? [56, 96] : kind === 'laptop' ? [96, 64] : [64, 96];
  const x = (160 - w) / 2;
  const y = (120 - hg) / 2;
  const on = active ? ' on' : '';
  return `<svg class="mt-art" viewBox="0 0 160 120" aria-hidden="true"><rect class="mt-chassis" x="${x}" y="${y}" width="${w}" height="${hg}" rx="4"/><circle class="mt-led${on}" cx="${x + 10}" cy="${y + 10}" r="3"/><circle class="mt-led${on}" cx="${x + 20}" cy="${y + 10}" r="3"/><line class="mt-vent" x1="${x + 8}" y1="${y + hg - 16}" x2="${x + w - 8}" y2="${y + hg - 16}"/></svg>`;
}

/** A CPU sparkline of the last 60 samples on a ruled baseline. */
function sparkSvg(hist: { cpu_percent: number }[]): Element | null {
  const v = hist.map((p) => p.cpu_percent).filter((x) => Number.isFinite(x)).slice(-60);
  if (!v.length) return null;
  const w = 160;
  const hgt = 48;
  const pts = v
    .map((y, i) => `${(v.length === 1 ? w / 2 : (i * w) / (v.length - 1)).toFixed(1)},${(hgt - 4 - (clamp(y) / 100) * (hgt - 8)).toFixed(1)}`)
    .join(' ');
  return svgOf(
    `<svg class="mt-spark" viewBox="0 0 ${w} ${hgt}" preserveAspectRatio="none" role="img" aria-label="CPU over the last samples"><line x1="0" y1="${hgt - 4}" x2="${w}" y2="${hgt - 4}"/><polyline points="${pts}"/></svg>`,
  );
}

function bar(p: number): HTMLElement {
  return h('div.nd-bar', null, h('i', { style: `width:${clamp(p)}%` }));
}

function tally(label: string, fig: string, cap: string, visual: Node | null): HTMLElement {
  return h('div.nd-tally', null, h('div.nd-tl', null, label), h('div.nd-fig', null, fig), visual, cap ? h('div.nd-cap', null, cap) : null);
}

/** A value to copy: a mono box with a Copy button. */
function copyBox(value: string): HTMLElement {
  return h('div.nd-box', null, h('code', null, value), button('Copy', () => void attempt(() => navigator.clipboard.writeText(value), 'Copied'), { kind: 'ghost', size: 'sm' }));
}

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  let alive = true;
  let ready = false;
  // null: the last request failed.
  let nodes: NodeView[] | null = null;
  let codes: JoinCode[] | null = null;
  let hub: HubIdentity | null = null;
  let link: NodeLink | null = null;
  // Each node's machine, joined by id (the hub is n0), and the agents' names by CID.
  let machines = new Map<string, MachineNode>();
  let agentsByCid = new Map<number, Agent>();
  // The open node's detail (one at a time, the hub's first), the machine runs with their VMs open, and the open dialog's closer.
  let openNode: string | null = HUB_NODE;
  const openRuns = new Set<string>();
  let closeOpen: (() => void) | null = null;

  const nodesBody = h('div');
  const codesBody = h('div');
  const hubBody = h('div');
  const linkBody = h('div.nd-body');
  const joinCode = input({ placeholder: 'Paste the join code', maxlength: 200, autocomplete: 'off', spellcheck: 'false', 'aria-label': 'Join code', class: 'node-code' });
  const joinError = h('div.nd-err');
  const joinButton = button('Join', () => void join(), { kind: 'primary' });
  const joinPane = h(
    'div.nd-join',
    null,
    field('Join code', joinCode, "Get a code from the other network's Nodes screen (Add a node)."),
    h('div.nd-acts', null, joinButton),
    joinError,
  );
  const joinedPane = h('div');
  const linkLede = h('p.muted', null, "Make this hub's machine a node of another network: it lends its machine to that network's agents. Its own plaza keeps running.");
  linkBody.append(linkLede, joinPane, joinedPane);

  const rebalanceBtn = button('Rebalance', () => void rebalance(), { title: 'Spread the idle agents over the machines in service, by memory' });
  const addBtn = button('Add a node', () => openAdd(), { kind: 'primary' });
  root.replaceChildren(
    pageHead('Nodes', [rebalanceBtn, addBtn]),
    card('Machines in this plaza', nodesBody, { flush: true }),
    card('Join another network', linkBody),
    card('Join codes', codesBody, { flush: true }),
    card('This hub', hubBody),
  );

  /** Opens a dialog and remembers its closer, so leaving the screen closes it. */
  const open = (title: string, body: Node[], foot: Node[] = []): (() => void) => {
    const close = dialog(title, body, foot, () => {
      closeOpen = null;
    });
    closeOpen = close;
    return close;
  };

  /** Draws the screen from what the hub answered (the copy first, then each fresh read); a null is a call that failed. */
  const paint = (n: NodeView[] | null, c: JoinCode[] | null, id: HubIdentity | null, ln: NodeLink | null, ms: MachineNode[] | null, ag: Agent[] | null): void => {
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
    paint(n, c, id, ln, ms, ag);
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
    const k = n.agents;
    const first =
      k === 0
        ? 'No new agent starts on it.'
        : `${k === 1 ? 'Its agent moves' : `Its ${k} agents move`} to the other machines in service, one at a time, and no new agent starts on it.`;
    const second =
      n.id === HUB_NODE
        ? 'This is the hub’s own machine: the hub keeps running, only its agents leave.'
        : 'Once it is stopped, its machine can be turned off. Resume brings it back, and agents move onto it again.';
    let close: () => void = () => {};
    close = open(
      `Stop ${n.name}?`,
      [
        h('p', null, first),
        h('p', null, second),
        h('p.muted', null, 'Stop now interrupts the turns in progress; each one resumes on its agent’s new machine. Otherwise an agent that is working moves when its turn ends.'),
      ],
      [
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
      ],
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
    let close: () => void = () => {};
    close = open(
      `Revoke ${n.name}?`,
      [h('p', null, 'Its signatures stop working at once and its agents go offline.')],
      [
        button('Cancel', () => close()),
        button(
          'Revoke',
          () => {
            close();
            void revoke(n);
          },
          { kind: 'danger' },
        ),
      ],
    );
  };

  // The node's buttons by its state: Resume or Stop, Move now while it stops, Revoke, or the hub's own note.
  const actionsOf = (n: NodeView): Node[] => {
    if (n.revoked) return [];
    const resumeBtn = button('Resume', () => void resume(n));
    let main: Node[];
    if (n.state === 'stopping') {
      main = [
        resumeBtn,
        button('Move now', () => void stop(n, true), { title: 'Interrupt the turns in progress: each resumes on its agent’s new machine' }),
      ];
    } else if (n.state === 'stopped' || n.draining) {
      main = [resumeBtn];
    } else {
      main = [button('Stop', () => confirmStop(n))];
    }
    const tail: Node[] = n.id === HUB_NODE ? [h('span.muted', null, 'this hub')] : [button('Revoke', () => confirmRevoke(n))];
    return [...main, ...tail];
  };

  // The run kills in a node's detail: then the page reloads.
  const killRun = async (id: string): Promise<void> => {
    await attempt(() => api.killRun(id), `Killed run ${id}`);
    void load();
  };

  const runsTable = (runs: NonNullable<MachineNode['runs']>): HTMLElement => {
    const rows = runs.map((r) => {
      const row = h(
        'tr',
        { class: 'clickable' },
        h('td', null, r.id),
        h('td', null, r.role || 'fleet'),
        h('td', null, badge(r.state, toneOf(r.state), true)),
        h('td.num', null, `${r.vms.length} VMs`),
        h('td.num', null, `started ${agoText(r.created_at)}`),
        h(
          'td.num',
          null,
          button('Kill', (ev) => {
            ev.stopPropagation();
            void killRun(r.id);
          }, { kind: 'danger', size: 'sm', icon: 'stop' }),
        ),
      );
      row.addEventListener('click', () => {
        if (openRuns.has(r.id)) openRuns.delete(r.id);
        else openRuns.add(r.id);
        renderNodes();
      });
      return row;
    });
    const head = h('tr', null, h('th', null, 'Run'), h('th', null, 'Role'), h('th', null, 'State'), h('th.num', null, 'VMs'), h('th.num', null, 'Started'), h('th.num', null, ''));
    return tableWrap(h('table.tbl', null, h('thead', null, head), h('tbody', null, rows)));
  };

  // The machine's live tallies, runs (a row opens its VMs) and technical details.
  const machineDetail = (m: MachineNode): Node[] => {
    const s = m.stats;
    const runs = m.runs ?? [];
    const out: Node[] = [h('p.nd-sentence', null, sentence(m))];
    if (s) {
      const cpuVisual = s.history?.length ? sparkSvg(s.history) : bar(s.cpu_percent);
      out.push(
        h(
          'div.nd-tallies',
          null,
          tally('CPU', `${Math.round(s.cpu_percent)}%`, '', cpuVisual),
          tally('Memory', `${gib(s.mem_used_mib)} / ${gib(s.mem_total_mib)}`, 'GiB used of GiB', bar((100 * s.mem_used_mib) / Math.max(s.mem_total_mib, 1))),
          tally('Load', (s.load || []).map(fx1).join(' · ') || '–', '1 / 5 / 15 min', null),
          tally("Agents' VMs", String(s.vms_running), `${gib(s.guest_mem_mib)} GiB reserved`, null),
        ),
        h('p.muted', null, `Up ${uptime(s.uptime_s)}`),
      );
    } else {
      out.push(h('p.muted', null, 'No live stats.'));
    }
    out.push(h('div.field-l', null, 'Runs'));
    out.push(runs.length ? runsTable(runs) : h('p.muted', null, 'No runs.'));
    for (const r of runs.filter((x) => openRuns.has(x.id))) {
      out.push(
        h(
          'div.nd-vms',
          null,
          h('strong', null, r.id),
          ...(r.vms.length
            ? r.vms.map((v) => h('div.nd-vm', null, `VM ${v.index} `, badge(v.state, toneOf(v.state), true), ' ', h('span.muted', null, `guest CID ${v.guest_cid ?? '—'}`)))
            : [h('p.muted', null, 'No VMs.')]),
        ),
      );
    }
    out.push(
      h(
        'details.nd-tech',
        null,
        h('summary', null, 'Technical details'),
        kv([
          ['Class', m.class || '—'],
          ['Arch', m.arch || '—'],
          ['Backend', m.backend || '—'],
          ['Irqchip', m.irqchip || '—'],
          ['vsock', m.vsock_mode || '—'],
          ['noded port', m.noded_port === undefined ? '—' : String(m.noded_port)],
        ]),
      ),
    );
    return out;
  };

  const nodeDetail = (n: NodeView, m: MachineNode | undefined): HTMLElement => {
    const mc = n.machine;
    const waits = waitingLines(n, (cid) => {
      const a = agentsByCid.get(cid);
      return a ? agentName(a) : undefined;
    });
    const lit = !!m && (m.runs ?? []).some((r) => r.vms.some((v) => LIVE_RE.test(v.state)));
    const view = m ? h('div.nd-view', null, svgOf(artSvg(m.kind, lit))) : null;
    return h(
      'div.nd-detail',
      null,
      view,
      ...(m ? machineDetail(m) : []),
      kv([
        ['Address', n.address || '—'],
        ['CIDs', cids(n)],
        ['Last seen', agoText(n.last_seen_ms)],
        ['Agents', String(n.agents)],
        ['Host', mc?.host ?? '—'],
        ['Version', mc?.version ?? '—'],
      ]),
      waits.length
        ? h('div.nd-wait', null, h('div.field-l', null, 'Waiting to move'), ...waits.map((line) => h('div', null, line)))
        : null,
      h('div.nd-acts', null, ...actionsOf(n)),
    );
  };

  // One node's block: its picture, name, state, facts; its detail opens under it.
  const nodeBlock = (n: NodeView): HTMLElement => {
    const m = machines.get(n.id);
    const st = nodeState(n);
    const live = m ? (m.runs ?? []).some((r) => r.vms.some((v) => LIVE_RE.test(v.state))) : n.online;
    const mc = n.machine;
    const machineLine = mc
      ? `${mc.cpus ?? '?'} cpus · ${Number.isFinite(mc.memory_mib) ? `${gib(mc.memory_mib)} GiB` : '?'} · ${mc.host || '?'}`
      : '—';
    const sub = m ? [KIND_LABEL[m.kind], m.model].filter(Boolean).join(' · ') : n.id;
    const facts: [string, string][] = [
      ['Address', n.address || '—'],
      ['Agents', String(n.agents)],
      ['CIDs', cids(n)],
      ['Machine', machineLine],
      ['Last seen', agoText(n.last_seen_ms)],
    ];
    const isOpen = openNode === n.id;
    const pic = h('span.nd-pic', { class: live ? 'live' : null }, svgOf(artSvg(m?.kind ?? 'workstation', live)));
    const head = h(
      'div.nd-head',
      { role: 'button', tabindex: '0', 'aria-expanded': String(isOpen), 'data-id': n.id },
      pic,
      h(
        'span.nd-main',
        null,
        h('span.nd-title', null, h('strong.nd-name', null, n.name), badge(st.label, st.tone, true)),
        h('span.nd-sub', null, sub),
        h('span.nd-grid', null, ...facts.map(([k, v]) => h('span.nd-fact', null, h('span', null, k), h('span', null, v)))),
      ),
    );
    const toggle = () => {
      openNode = openNode === n.id ? null : n.id;
      renderNodes();
    };
    head.addEventListener('click', toggle);
    head.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        toggle();
      }
    });
    return h('div.nd-node', null, head, isOpen ? nodeDetail(n, m) : null);
  };

  const renderNodes = (): void => {
    if (!ready) return;
    if (nodes === null) return replace(nodesBody, h('div.nd-msg', null, 'Could not load the machines.'));
    if (!nodes.length) {
      return replace(
        nodesBody,
        empty('machine', 'No machines yet', 'Add a node here to bring a machine into this network, or join this hub to another network below.'),
      );
    }
    replace(nodesBody, ...nodes.map(nodeBlock));
  };

  const renderCodes = (): void => {
    if (!ready) return;
    if (codes === null) return replace(codesBody, h('div.nd-msg', null, 'Could not load the join codes.'));
    if (!codes.length) {
      return replace(codesBody, empty('key', 'No join codes yet', 'Codes made with "Add a node" are listed here.'));
    }
    const rows = [...codes].sort((a, b) => b.created_ms - a.created_ms);
    const body = h(
      'table.tbl',
      null,
      h('thead', null, h('tr', null, h('th', null, 'Label'), h('th', null, 'ID'), h('th', null, 'Created'), h('th', null, 'Expires'), h('th', null, 'State'), h('th', null, ''))),
      h(
        'tbody',
        null,
        rows.map((c) => {
          const [label, tone] = JOIN_CODE[c.state];
          return h(
            'tr',
            null,
            h('td', null, h('strong', null, c.label)),
            h('td', null, h('span.muted', null, c.id)),
            h('td', null, agoText(c.created_ms)),
            h(
              'td',
              null,
              c.state === 'active' ? h('span', null, 'in ', h('span', { 'data-end': String(c.expires_ms) }, mmss(c.expires_ms - Date.now()))) : agoText(c.expires_ms),
            ),
            h('td', null, badge(label, tone, true), c.used_by ? h('span.muted', null, ` by ${c.used_by}`) : null),
            h('td', null, c.state === 'active' ? button('Revoke', () => void revokeCode(c), { size: 'sm' }) : null),
          );
        }),
      ),
    );
    replace(codesBody, tableWrap(body));
  };

  const renderHub = (): void => {
    if (!ready) return;
    if (!hub) return replace(hubBody, h('div.nd-msg', null, 'Could not load this hub.'));
    const id: HubIdentity = hub;
    replace(
      hubBody,
      h(
        'div.nd-body',
        null,
        kv([
          ['Name', id.name],
          ['URL', id.url],
          ['Version', id.version],
          ['TLS fingerprint', id.fingerprint || 'no TLS (loopback only)'],
        ]),
        h('div.nd-acts', null, button('Rotate owner token', () => confirmRotate(), { kind: 'danger' })),
      ),
    );
  };

  const confirmRotate = (): void => {
    const slot = h('div.vlt-form', null, h('p', null, 'The current owner token stops working at once.'));
    let close: () => void = () => {};
    const rotate = async (): Promise<void> => {
      const made = await attempt(() => api.rotateOwnerToken());
      if (!made) return;
      replace(slot, h('p', null, 'The new owner token, shown once:'), copyBox(made.token), h('p.muted', null, 'Every signed-in browser must sign in again.'));
      slot.closest('.sheet')?.querySelector('footer.sheet-f')?.remove();
      void load();
    };
    close = open('Rotate the owner token?', [slot], [button('Cancel', () => close()), button('Rotate', () => void rotate(), { kind: 'danger' })]);
  };

  // ---- Join another network: this hub lends its machine to that network ----

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

  const linkFacts = (l: NodeLink): HTMLElement =>
    h(
      'div.nd-body',
      null,
      kv([
        ['Hub', l.hub ?? '—'],
        ['This node', l.node_id ?? '—'],
        ['CIDs', linkCids(l.cids)],
        ['Status', h('span', null, badge(l.running ? 'running' : 'stopped', l.running ? 'ok' : 'muted', true), l.running && l.pid ? h('span.muted', null, ` pid ${l.pid}`) : null)],
      ]),
      h('div.nd-acts', null, button('Leave', () => confirmLeave())),
    );

  const renderLink = (): void => {
    if (!ready) return;
    const l = link;
    if (!l) {
      joinPane.hidden = true;
      joinedPane.hidden = false;
      replace(joinedPane, h('div.nd-msg', null, 'Could not load the network link.'));
      return;
    }
    joinPane.hidden = l.joined;
    joinedPane.hidden = !l.joined;
    if (l.joined) replace(joinedPane, linkFacts(l));
  };

  const leave = async (forget: boolean): Promise<void> => {
    const status = await attempt(() => api.leaveNodeLink(forget), 'Left the network');
    if (!status) return;
    link = status;
    renderLink();
  };

  const confirmLeave = (): void => {
    const forget = h('input', { type: 'checkbox' }) as HTMLInputElement;
    let close: () => void = () => {};
    close = open(
      'Leave this network?',
      [
        h(
          'div.nd-body',
          null,
          h('p', null, 'This node stops lending its machine to that network, and its agents there go offline.'),
          h('label.nd-check', null, forget, h('span', null, 'Forget this network: remove the saved hub, so joining again needs a new code.')),
        ),
      ],
      [
        button('Cancel', () => close()),
        button(
          'Leave',
          () => {
            close();
            void leave(forget.checked);
          },
          { kind: 'danger' },
        ),
      ],
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
    const slot = h('div.vlt-form', null, field('Label', label, 'What the machine is called here.'), field('Valid for', minutes, 'How long the code can be used to join.'));
    const create = async (): Promise<void> => {
      const name = label.value.trim();
      if (!name) return toast('Give the machine a label first.', 'error');
      const made = await attempt(() => api.issueJoinCode(name, Number(minutes.value)));
      if (!made) return;
      replace(
        slot,
        h('p.nd-warn', null, 'Shown once. Single use. Anyone with it can join until it expires.'),
        copyBox(made.token),
        h('p.muted', null, 'Expires in ', h('span', { 'data-end': String(made.expires_ms) }, mmss(made.expires_ms - Date.now()))),
        h('p.muted', null, 'On the new machine, run:'),
        copyBox(`apz-link join ${made.token}`),
        h('p.muted', null, "or paste it into the desktop app's Join a hub."),
      );
      slot.closest('.sheet')?.querySelector('footer.sheet-f')?.remove();
      void load();
    };
    open('Add a node', [slot], [button('Create join code', () => void create(), { kind: 'primary' })]);
  };

  // Counts every join-code expiry on the screen, once a second.
  const tickEnds = (): void => {
    document.querySelectorAll<HTMLElement>('[data-end]').forEach((el) => {
      el.textContent = mmss(Number(el.dataset.end) - Date.now());
    });
  };

  // The last read paints at once; the reads that follow (each a 304 when nothing changed) replace it.
  const kept = { nodes: api.nodes.cached(), codes: api.joinCodes.cached(), hub: api.hubIdentity.cached(), link: api.nodeLink.cached(), machines: api.machines.cached() };
  if (kept.nodes !== undefined) paint(kept.nodes, kept.codes ?? null, kept.hub ?? null, kept.link ?? null, kept.machines ?? null, api.agents.cached() ?? null);
  void load();
  const refresh = window.setInterval(() => void load(), REFRESH_MS);
  const clock = window.setInterval(tickEnds, 1000);
  const off = on(RESYNC, () => void load());

  return () => {
    alive = false;
    off();
    window.clearInterval(refresh);
    window.clearInterval(clock);
    closeOpen?.();
  };
}
