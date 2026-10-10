/* The roster: who is running right now. Every VM the register names a role
   for — the long-running role agents a node boots at its initial setup, one
   per role of docs/agents/initial-roles.json — grouped by role and read from
   the same store as every other view: the state and guest CID are the
   register's, the line count is the same serial cache the run entry reads,
   and lines/s is the same activity tracker the map and 3D view pulse from.
   A VM the register gives no role is not a roster row, and a register that
   names no roles gets the honest hint, never a placeholder fleet.

   The hub (the vsock daemon that serves those agents) is joined in here by
   guest CID: each register row gains the hub's facts for the same CID —
   connected at, heartbeat age, calls — and the two sides are honest when
   only one knows an agent (`connected, not registered` / `registered, not
   connected`), never silently merged. A hub the console cannot read says so
   and claims nothing; that is distinct from a hub that answers with nothing. */

import { announce, byId, el } from '../dom';
import { fmtDur, fmtNum, fmtStamp } from '../format';
import { vmKey } from '../models/activity';
import type { HubJoinRow } from '../models/hub';
import { joinHub } from '../models/hub';
import type { Run, Vm, VmState } from '../models/run';
import { isOpen, roleOf, seqOf } from '../models/run';
import { store } from '../store';
import { roleChip, renderHubLine, vmStateMk } from './chrome';

export interface RoleAgent {
  role: string;
  runId: string;
  vmId: number;
  cid: number | null;
  state: VmState;
  /** Whether the agent's run is still open (its VM may still be alive). */
  open: boolean;
  lines: number;
  lps: number;
  /** When the agent's run opened, in the node's clock. */
  since: number;
}

export interface RoleGroup {
  role: string;
  agents: RoleAgent[];
}

export interface RosterRead {
  groups: RoleGroup[];
  /** Role VMs on the register, and how many of them are running now. */
  agents: number;
  running: number;
  roles: number;
}

function byCid(a: RoleAgent, b: RoleAgent): number {
  const ac = a.cid ?? Number.MAX_SAFE_INTEGER;
  const bc = b.cid ?? Number.MAX_SAFE_INTEGER;
  if (ac !== bc) return ac - bc;
  if (a.runId !== b.runId) return seqOf(a.runId) - seqOf(b.runId);
  return a.vmId - b.vmId;
}

/** The register read as a roster: every role VM, grouped by role (roles in
    alphabetical order, agents by guest CID). Pure — the caller supplies the
    line and rate readings, so the harness can test the grouping with no
    daemon and no DOM. */
export function readRoster(
  runs: readonly Run[],
  lines: (run: Run, vm: Vm) => number,
  rate: (run: Run, vm: Vm) => number,
): RosterRead {
  const byRole = new Map<string, RoleAgent[]>();
  let agents = 0;
  let running = 0;
  for (const run of runs) {
    for (const vm of run.vms) {
      const role = roleOf(vm);
      if (role == null) continue;
      agents++;
      if (vm.state === 'running') running++;
      const agent: RoleAgent = {
        role,
        runId: run.id,
        vmId: vm.id,
        cid: vm.guest_cid ?? null,
        state: vm.state,
        open: isOpen(run),
        lines: lines(run, vm),
        lps: rate(run, vm),
        since: run.created_at,
      };
      const group = byRole.get(role);
      if (group) group.push(agent);
      else byRole.set(role, [agent]);
    }
  }
  const groups = [...byRole.entries()]
    .map(([role, list]) => ({ role, agents: list.sort(byCid) }))
    .sort((a, b) => (a.role < b.role ? -1 : a.role > b.role ? 1 : 0));
  return { groups, agents, running, roles: groups.length };
}

/* --------------------------------------------------------- the joined fleet
   The register's role rows joined to the hub's connected agents by CID, then
   grouped by role for the table. When the hub cannot be read, no presence is
   claimed — every register row stands alone with a dash in the hub's
   columns, which is not the same as "registered, not connected". */
type FleetGroup = { role: string; rows: Array<HubJoinRow<RoleAgent>> };

function groupFleet(rows: Array<HubJoinRow<RoleAgent>>): FleetGroup[] {
  const byRole = new Map<string, Array<HubJoinRow<RoleAgent>>>();
  for (const row of rows) {
    const role = row.role ?? '—';
    const list = byRole.get(role);
    if (list) list.push(row);
    else byRole.set(role, [row]);
  }
  return [...byRole.entries()]
    .map(([role, list]) => ({
      role,
      rows: list.sort((x, y) => (x.cid ?? Number.MAX_SAFE_INTEGER) - (y.cid ?? Number.MAX_SAFE_INTEGER)),
    }))
    .sort((a, b) => (a.role < b.role ? -1 : a.role > b.role ? 1 : 0));
}

/** One row's live readings, updated in place so focus and scroll survive. */
interface AgentRow {
  lines: Text | null;
  lps: Text | null;
  up: Text | null;
  beat: Text | null;
  since: number | null;
}

let rosterSig: string | null = null;
let agentRows = new Map<string, AgentRow>();
let announced: string | null = null;

export function renderRoster(): void {
  const notice = byId('roster-notice');
  if (store.runsError != null && store.runs.length === 0) {
    notice.hidden = false;
    byId('roster-notice-text').textContent =
      `The node daemon is not answering at ${location.origin} (${store.runsError}). Retrying; the roster catches up on its own.`;
  } else {
    notice.hidden = true;
  }
  renderHubLine('roster-hub-line');

  /* only the single-run GET carries serial, so the roster reads every entry
     through the store's freshest copy of it, exactly as the run entry does */
  const read = readRoster(
    store.runs.map((r) => store.detailOf(r)),
    (run, vm) => store.linesFor(run.id, vm.id, vm.serial).length,
    (run, vm) => store.activity.get(vmKey(run.id, vm.id)) ?? 0,
  );
  const live = byId('roster-live');
  live.hidden = store.health !== 'live';
  live.textContent = store.health;
  byId('t-roles').textContent = fmtNum(read.roles);
  byId('t-agents').textContent = fmtNum(read.agents);
  byId('t-running').textContent = fmtNum(read.running);
  const registerVms = store.runs.reduce((n, r) => n + r.vms.length, 0);
  byId('t-register-vms').textContent = store.runsError != null ? '—' : fmtNum(registerVms);

  /* the join: only when the hub answered this cycle. On a hub we cannot read,
     the register's own rows stand alone and the hub's columns read "—". */
  const hubReadable = store.hubError == null && store.hub != null;
  const registered = read.groups.flatMap((g) => g.agents);
  const rows: Array<HubJoinRow<RoleAgent>> = hubReadable
    ? joinHub(registered, store.hub?.agents ?? [])
    : registered.map((r) => ({ cid: r.cid, role: r.role, presence: 'registered-only' as const, registered: r, connected: null }));
  const groups = groupFleet(rows);
  const connectedCount = hubReadable ? (store.hub?.agents.length ?? 0) : 0;

  const has = rows.length > 0;
  byId('roster-c').textContent = has
    ? `${fmtNum(read.running)} of ${fmtNum(read.agents)} running` +
      (hubReadable ? ` · ${fmtNum(connectedCount)} connected on the hub` : '')
    : 'no roles named yet';

  const empty = byId('roster-empty');
  const table = byId('roster-table');
  empty.hidden = has || store.runsError != null;
  table.hidden = !has;
  if (!has) {
    empty.textContent =
      'The register names no roles and the hub serves no agents yet. A node boots its role fleet at ' +
      'startup (`noded --agents docs/agents/initial-roles.json` — one long-running agent per role); ' +
      'each agent appears here as soon as the register carries its role or the hub reports its connection.';
    rosterSig = null;
    agentRows = new Map();
    byId('roster-body').textContent = '';
    return;
  }

  const hubSig = hubReadable
    ? (store.hub?.agents ?? []).map((a) => `${a.cid}/${a.role}/${a.connected_at_ms}/${a.calls}`).join(',')
    : `unreadable:${store.hubError ?? ''}`;
  const sig = groups
    .map((g) => `${g.role}:${g.rows.map((r) => rowKey(r) + '/' + r.presence + '/' + (r.connected ? 'c' : '') + '/' + (r.registered ? `${r.registered.state}/${r.registered.open}/${r.registered.since}` : '')).join(',')}`)
    .join('|') + '#' + hubSig;
  if (sig !== rosterSig) {
    rosterSig = sig;
    agentRows = new Map();
    rebuildBody(groups, hubReadable);
    const summary =
      `${fmtNum(read.agents)} role ${read.agents === 1 ? 'agent' : 'agents'} across ${fmtNum(read.roles)} ` +
      `${read.roles === 1 ? 'role' : 'roles'}, ${fmtNum(read.running)} running` +
      (hubReadable ? `, ${fmtNum(connectedCount)} connected on the hub.` : '.');
    if (summary !== announced) {
      announced = summary;
      announce(summary);
    }
  }

  /* the live readings tick in place, so focus and the scroll position stay */
  const now = Date.now();
  for (const group of groups) {
    for (const row of group.rows) {
      const refs = agentRows.get(rowKey(row));
      if (!refs) continue;
      if (refs.lines && row.registered) refs.lines.nodeValue = fmtNum(row.registered.lines);
      if (refs.lps && row.registered) refs.lps.nodeValue = row.registered.lps > 0.05 ? row.registered.lps.toFixed(1) : '—';
      if (refs.up && refs.since != null) refs.up.nodeValue = fmtDur(now - refs.since);
      if (refs.beat && row.connected) refs.beat.nodeValue = fmtDur(Math.max(0, now - row.connected.last_heartbeat_ms));
    }
  }
}

/** A row's stable identity: a register row by its run/vm, a hub-only row by
    its CID (there is no register entry to name). */
function rowKey(row: HubJoinRow<RoleAgent>): string {
  return row.registered ? `${row.registered.runId}/${row.registered.vmId}` : `hub:${row.cid ?? '—'}`;
}

/** Build the groups: one header row per role, then its agents, the register's
    row shape with the hub's facts beside it. */
function rebuildBody(groups: FleetGroup[], hubReadable: boolean): void {
  const body = byId('roster-body');
  body.textContent = '';
  let n = 0;
  for (const group of groups) {
    const head = el('tr', 'grp');
    const cell = el('td', null);
    cell.colSpan = 9;
    cell.appendChild(el('span', 'role', group.role));
    const running = group.rows.filter((r) => r.registered?.state === 'running').length;
    const connected = group.rows.filter((r) => r.connected != null).length;
    cell.appendChild(el('span', 'c',
      `${group.rows.length} ${group.rows.length === 1 ? 'agent' : 'agents'} · ${running} running` +
      (connected > 0 ? ` · ${connected} connected` : '')));
    head.appendChild(cell);
    body.appendChild(head);
    for (const row of group.rows) {
      n++;
      const reg = row.registered;
      const tr = el('tr', 'link');
      if (reg) {
        tr.tabIndex = 0;
        tr.setAttribute('data-go', `#run/${reg.runId}`);
      }
      const where = [
        row.role ?? '—',
        reg ? `run ${reg.runId} vm ${reg.vmId}` : 'no register entry',
        row.cid != null ? `cid ${row.cid}` : 'no cid',
        row.presence === 'both' ? 'connected and registered' : row.presence === 'connected-only' ? 'connected, not registered' : 'registered, not connected',
      ].join(', ');
      tr.setAttribute('aria-label', where);
      tr.appendChild(el('td', 'num', String(n)));
      const roleTd = el('td');
      const chip = roleChip(row.role);
      if (chip) roleTd.appendChild(chip);
      tr.appendChild(roleTd);
      tr.appendChild(el('td', 'm', reg ? `${reg.runId} · vm ${reg.vmId}` : '—'));
      tr.appendChild(el('td', 'm', row.cid != null ? `cid ${row.cid}` : '—'));

      /* the hub cell: the facts when connected, the honest label when only
         one side knows the agent, a dash when the hub cannot be read */
      const hubTd = el('td', 'm');
      let beat: Text | null = null;
      if (row.connected == null) {
        hubTd.textContent = hubReadable ? 'registered, not connected' : '—';
      } else {
        const a = row.connected;
        const lead = row.presence === 'connected-only' ? 'connected, not registered · ' : 'connected ';
        hubTd.appendChild(document.createTextNode(`${lead}${fmtStamp(a.connected_at_ms)} · hb `));
        beat = document.createTextNode(fmtDur(Math.max(0, Date.now() - a.last_heartbeat_ms)));
        hubTd.appendChild(beat);
        const note = reg && reg.role !== a.role ? ` · hub names "${a.role}"` : '';
        hubTd.appendChild(document.createTextNode(` · ${fmtNum(a.calls)} calls${note}`));
      }
      tr.appendChild(hubTd);

      const st = el('td');
      if (reg) {
        st.appendChild(vmStateMk(reg.state));
        if (!reg.open) st.appendChild(el('span', 'c', ` · closed`));
      } else {
        st.textContent = '—';
      }
      tr.appendChild(st);

      const linesTd = el('td', 'num');
      const lines = document.createTextNode(reg ? fmtNum(reg.lines) : '—');
      linesTd.appendChild(lines);
      const lpsTd = el('td', 'num');
      const lps = document.createTextNode(reg && reg.lps > 0.05 ? reg.lps.toFixed(1) : '—');
      lpsTd.appendChild(lps);
      const upTd = el('td', 'm');
      const up = document.createTextNode(reg ? fmtDur(Date.now() - reg.since) : '—');
      upTd.appendChild(up);
      tr.appendChild(linesTd);
      tr.appendChild(lpsTd);
      tr.appendChild(upTd);
      body.appendChild(tr);
      agentRows.set(rowKey(row), {
        lines: reg ? lines : null,
        lps: reg ? lps : null,
        up: reg ? up : null,
        beat,
        since: reg ? reg.since : null,
      });
    }
  }
}

/** The roster's own wiring: a row opens its run's entry, the register's rule.
    A hub-only row has no entry to open, so it stays inert. */
export function initRoster(): void {
  byId('roster-body').addEventListener('click', (e) => {
    const row = (e.target as HTMLElement | null)?.closest<HTMLElement>('tr[data-go]');
    if (!row) return;
    const dest = row.getAttribute('data-go');
    if (dest) location.hash = dest;
  });
}
