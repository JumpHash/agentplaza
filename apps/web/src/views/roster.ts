/* The roster: who is running right now. Every VM the register names a role
   for — the long-running role agents a node boots at its initial setup, one
   per role of docs/agents/initial-roles.json — grouped by role and read from
   the same store as every other view: the state and guest CID are the
   register's, the line count is the same serial cache the run entry reads,
   and lines/s is the same activity tracker the map and 3D view pulse from.
   A VM the register gives no role is not a roster row, and a register that
   names no roles gets the honest hint, never a placeholder fleet. */

import { announce, byId, el } from '../dom';
import { fmtDur, fmtNum } from '../format';
import { vmKey } from '../models/activity';
import type { Run, Vm, VmState } from '../models/run';
import { isOpen, roleOf, seqOf } from '../models/run';
import { store } from '../store';
import { roleChip, vmStateMk } from './chrome';

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

/* ------------------------------------------------------------ rendering */
interface AgentRow {
  lines: Text;
  lps: Text;
  up: Text;
  since: number;
}

let rosterSig: string | null = null;
let agentRows: Record<string, AgentRow> = {};
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

  const empty = byId('roster-empty');
  const table = byId('roster-table');
  const has = read.agents > 0;
  byId('roster-c').textContent = has
    ? `${fmtNum(read.running)} of ${fmtNum(read.agents)} running`
    : 'no roles named yet';
  empty.hidden = has || store.runsError != null;
  table.hidden = !has;
  if (!has) {
    empty.textContent =
      'The register names no roles yet. A node boots its role fleet at startup ' +
      '(`noded --agents docs/agents/initial-roles.json` — one long-running agent per role); ' +
      'each agent appears here, grouped under its role, as soon as the register carries the field.';
    rosterSig = null;
    agentRows = {};
    byId('roster-body').textContent = '';
    return;
  }

  const sig = read.groups
    .map((g) => `${g.role}:${g.agents.map((a) => `${a.runId}/${a.vmId}/${a.cid ?? ''}/${a.state}/${a.open ? 'open' : 'shut'}/${a.since}`).join(',')}`)
    .join('|');
  if (sig !== rosterSig) {
    rosterSig = sig;
    agentRows = {};
    rebuildBody(read);
    const summary = `${fmtNum(read.agents)} role ${read.agents === 1 ? 'agent' : 'agents'} across ${fmtNum(read.roles)} ${read.roles === 1 ? 'role' : 'roles'}, ${fmtNum(read.running)} running.`;
    if (summary !== announced) {
      announced = summary;
      announce(summary);
    }
  }

  /* the live readings tick in place, so focus and the scroll position stay */
  const now = Date.now();
  for (const group of read.groups) {
    for (const agent of group.agents) {
      const row = agentRows[`${agent.runId}/${agent.vmId}`];
      if (!row) continue;
      row.lines.nodeValue = fmtNum(agent.lines);
      row.lps.nodeValue = agent.lps > 0.05 ? agent.lps.toFixed(1) : '—';
      row.up.nodeValue = fmtDur(now - agent.since);
    }
  }
}

/** Build the groups: one header row per role, then its agents. */
function rebuildBody(read: RosterRead): void {
  const body = byId('roster-body');
  body.textContent = '';
  let n = 0;
  for (const group of read.groups) {
    const head = el('tr', 'grp');
    const cell = el('td', null);
    cell.colSpan = 8;
    cell.appendChild(el('span', 'role', group.role));
    const running = group.agents.filter((a) => a.state === 'running').length;
    cell.appendChild(el('span', 'c',
      `${group.agents.length} ${group.agents.length === 1 ? 'agent' : 'agents'} · ${running} running`));
    head.appendChild(cell);
    body.appendChild(head);
    for (const agent of group.agents) {
      n++;
      const tr = el('tr', 'link');
      tr.tabIndex = 0;
      tr.setAttribute('data-go', `#run/${agent.runId}`);
      tr.setAttribute('aria-label',
        `${agent.role}, run ${agent.runId} vm ${agent.vmId}, ${agent.state}` +
        `${agent.cid != null ? `, cid ${agent.cid}` : ''}`);
      tr.appendChild(el('td', 'num', String(n)));
      const roleTd = el('td');
      const chip = roleChip(agent.role);
      if (chip) roleTd.appendChild(chip);
      tr.appendChild(roleTd);
      tr.appendChild(el('td', 'm', `${agent.runId} · vm ${agent.vmId}`));
      tr.appendChild(el('td', 'm', agent.cid != null ? `cid ${agent.cid}` : '—'));
      const st = el('td');
      st.appendChild(vmStateMk(agent.state));
      if (!agent.open) st.appendChild(el('span', 'c', ` · closed`));
      tr.appendChild(st);
      const linesTd = el('td', 'num');
      const lines = document.createTextNode('');
      linesTd.appendChild(lines);
      const lpsTd = el('td', 'num');
      const lps = document.createTextNode('');
      lpsTd.appendChild(lps);
      const upTd = el('td', 'm');
      const up = document.createTextNode('');
      upTd.appendChild(up);
      tr.appendChild(linesTd);
      tr.appendChild(lpsTd);
      tr.appendChild(upTd);
      body.appendChild(tr);
      agentRows[`${agent.runId}/${agent.vmId}`] = { lines, lps, up, since: agent.since };
    }
  }
}

/** The roster's own wiring: a row opens its run's entry, the register's rule. */
export function initRoster(): void {
  byId('roster-body').addEventListener('click', (e) => {
    const row = (e.target as HTMLElement | null)?.closest<HTMLElement>('tr[data-go]');
    if (!row) return;
    const dest = row.getAttribute('data-go');
    if (dest) location.hash = dest;
  });
}
