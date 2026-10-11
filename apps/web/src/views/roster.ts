/* The roster: who is running right now. Every VM the register names a role
   for — the long-running role agents a node boots at its initial setup, one
   per role of docs/agents/initial-roles.json — grouped by role and read from
   the same store as every other view: the state and guest CID are the
   register's, the line count is the same serial cache the run entry reads,
   and lines/s is the same activity tracker the map and the floor draw from.
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
import type { StopState } from '../models/stop';
import { parseStopReason } from '../models/stop';
import type { TellState } from '../models/tell';
import { parseTellArgs } from '../models/tell';
import type { StopRead, TellRead } from '../store';
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

/* The tool console: the agent whose console is open (its guest CID), the row's
   drive buttons (so the open one can be marked), the index of the recent call
   being read (0 = the newest), and the last reading announced to a screen
   reader. It is view state — the store keeps the calls themselves. */
let consoleCid: number | null = null;
let consoleIdx = 0;
let driveRefs = new Map<number, HTMLButtonElement>();
let consoleAnnounced = '';
let consolePastSig: string | null = null;
let consolePastRefs: Array<{ idx: number; btn: HTMLButtonElement }> = [];
/* The stop affordance: whether the console's stop is armed (the two-step,
   never-a-single-click press) and the last reading announced to a screen
   reader. Armed state is view state, and leaving the card — closing it, or
   opening another agent — disarms it. */
let stopArmed = false;
let stopAnnounced = '';

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
    empty.textContent = 'The register names no roles and the hub serves no agents yet.';
    rosterSig = null;
    agentRows = new Map();
    byId('roster-body').textContent = '';
    driveRefs = new Map();
    renderConsole([]);
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

  renderConsole(rows);
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
  driveRefs = new Map();
  let n = 0;
  for (const group of groups) {
    const head = el('tr', 'grp');
    const cell = el('td', null);
    cell.colSpan = 10;
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

      /* the drive cell: the console is addressed by guest CID (the endpoint
         is), so only a row that names one can be driven; a row without a CID
         says so rather than offering a button that cannot work */
      const driveTd = el('td');
      if (row.cid != null) {
        const cid = row.cid;
        const btn = el('button', 'btn sm drive');
        btn.type = 'button';
        btn.setAttribute('data-drive', String(cid));
        btn.textContent = 'Drive';
        driveTd.appendChild(btn);
        driveRefs.set(cid, btn);
      } else {
        driveTd.textContent = '—';
      }
      tr.appendChild(driveTd);

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

/* ---------------------------------------------------------- the tool console
   The operator's hand on one agent: a tool name (free text — the API serves no
   grant list), a JSON object of arguments and a timeout, posted to the node's
   `/tell` proxy the moment the owner sends it, and only then. The answer is the
   agent's own — `ok` plus its `output` — and the output is guest-derived, so it
   lands in the ledger's ruled, bounded, text-only box, never as markup. A
   refusal is the daemon's, the hub's or the agent's own words, verbatim, under
   the mark its status earns. */

/** The mark each ending wears, in the register's own language: the ok roundel
    for the agent's answer (amber when the tool itself declined), an outlined
    crit mark for a refusal, amber for a node or hub that cannot read. */
const TELL_MARK: Readonly<Record<TellState, { cls: string; label: string }>> = {
  driving: { cls: 'mk warn', label: 'driving' },
  sent: { cls: 'mk ok', label: 'answered' },
  policy: { cls: 'mk crit', label: 'role policy' },
  'not-connected': { cls: 'mk crit', label: 'not connected' },
  timeout: { cls: 'mk warn', label: 'no answer in time' },
  off: { cls: 'mk warn', label: 'hub admin off' },
  unreachable: { cls: 'mk warn', label: 'unreachable' },
  refused: { cls: 'mk crit', label: 'refused' },
};

/** The mark a driven call wears: the agent's own answer is ok, or — when the
    tool refused — the agent's decline; every other ending is its own state. */
function tellMark(read: TellRead): { cls: string; label: string } {
  if (read.state === 'sent') {
    return read.ok ? TELL_MARK.sent : { cls: 'mk warn', label: 'the tool declined' };
  }
  return TELL_MARK[read.state];
}

/** A driven call's own line: the tool and its wait, then the answer's
    provenance or the status that names the refusal. */
function tellReading(read: TellRead): string {
  const call = `${read.name} · ${read.timeoutMs} ms`;
  if (read.state === 'driving') return `${call} · waiting for the agent …`;
  if (read.state === 'sent') return `${call} · cid ${read.cid} · the agent's own answer`;
  return read.status == null ? `${call} · the node did not answer` : `${call} · HTTP ${read.status}`;
}

/** Open the console on one agent: the row's drive button, and the owner's own
    hand from there on. */
function openConsole(cid: number): void {
  consoleCid = cid;
  consoleIdx = 0;
  consoleAnnounced = '';
  /* opening a console is leaving the last card: a stop armed on another agent
     is never carried over (the owner's press names one session) */
  stopArmed = false;
  stopAnnounced = '';
  byId('roster-console').hidden = false;
  const args = byId<HTMLTextAreaElement>('rc-args');
  if (args.value.trim() === '') args.value = '{}';
  byId('rc-form-err').textContent = '';
  byId('rc-stop-form-err').textContent = '';
  byId<HTMLInputElement>('rc-stop-reason').value = '';
  store.scheduleRender();
  byId<HTMLInputElement>('rc-name').focus();
}

function closeConsole(): void {
  consoleCid = null;
  consoleIdx = 0;
  stopArmed = false; // leaving the card disarms the stop
  store.scheduleRender();
}

/** Send the console's form: the tool name, its arguments and the timeout. The
    arguments are parsed here — a box that is not a JSON object is named at the
    field, rather than round-tripped as the daemon's 400. */
function sendConsole(event: Event): void {
  event.preventDefault();
  const err = byId('rc-form-err');
  if (consoleCid == null) {
    err.textContent = 'open a console on an agent first';
    return;
  }
  const name = byId<HTMLInputElement>('rc-name').value.trim();
  if (name === '') {
    err.textContent = 'the call needs a tool name';
    return;
  }
  const parsed = parseTellArgs(byId<HTMLTextAreaElement>('rc-args').value);
  if ('error' in parsed) {
    err.textContent = parsed.error;
    return;
  }
  const timeoutMs = Number(byId<HTMLInputElement>('rc-timeout').value.trim());
  if (!Number.isInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 5000) {
    err.textContent = 'the timeout must be a whole number of 1 to 5000 ms';
    return;
  }
  err.textContent = '';
  consoleIdx = 0;
  consoleAnnounced = '';
  void store.driveTool(consoleCid, name, parsed.args, timeoutMs);
}

/** The console, for whichever agent it is open on. A console whose agent has
    left the fleet closes itself: there is no row to drive. */
function renderConsole(rows: readonly HubJoinRow<RoleAgent>[]): void {
  const panel = byId('roster-console');
  const row = consoleCid == null ? null : (rows.find((r) => r.cid === consoleCid) ?? null);
  const cid = row?.cid ?? null;
  if (row == null || cid == null) {
    if (consoleCid != null) {
      consoleCid = null;
      consoleIdx = 0;
    }
    panel.hidden = true;
    return;
  }
  panel.hidden = false;
  for (const [refCid, btn] of driveRefs) {
    if (refCid === consoleCid) btn.setAttribute('aria-pressed', 'true');
    else btn.removeAttribute('aria-pressed');
  }
  const role = row.role ?? row.connected?.role ?? null;
  const presence = row.presence === 'connected-only' ? ' · connected, not on the register' : '';
  byId('rc-who').textContent = `cid ${cid}${role ? ` · ${role}` : ''}${presence}`;

  /* the calls the store holds for this agent; the one still driving, if any,
     is the newest and is shown in the result box rather than as history */
  const all = store.tells.get(cid) ?? [];
  const settled = all.filter((t) => t.state !== 'driving');
  const pending = all.find((t) => t.state === 'driving') ?? null;
  const read = consoleIdx === 0 && pending != null ? pending : (settled[consoleIdx] ?? settled[0] ?? null);
  renderConsoleResult(read);
  renderConsolePast(cid, settled);
  renderStop(row, cid);
}

function renderConsoleResult(read: TellRead | null): void {
  const body = byId('rc-result');
  if (read == null) {
    body.hidden = true;
    return;
  }
  body.hidden = false;
  byId('rc-tool').textContent = read.name;
  const mk = tellMark(read);
  const mark = byId('rc-mark');
  mark.className = mk.cls;
  mark.textContent = mk.label;
  byId('rc-meta').textContent = tellReading(read);

  const err = byId('rc-err');
  err.hidden = read.error === '';
  err.textContent = read.error;

  const text = byId<HTMLPreElement>('rc-text');
  text.hidden = read.state !== 'sent';
  text.textContent = read.state === 'sent' ? read.output : '';

  /* a screen reader hears each answer once, and only answers — the pending
     state is the press it already made */
  if (read.state !== 'driving') {
    const key = `${read.cid}\u0000${read.at}\u0000${read.state}`;
    if (key !== consoleAnnounced) {
      consoleAnnounced = key;
      announce(
        read.state === 'sent'
          ? `The agent at cid ${read.cid} answered the ${read.name} call${read.ok ? '' : ' with a refusal'}.`
          : `${read.name} on the agent at cid ${read.cid} was ${mk.label}: ${read.error}`,
      );
    }
  }
}

/** The recent calls to this agent, newest first: each is a button that brings
    its own result back into the box, so an earlier call can be read again
    without driving it a second time. */
function renderConsolePast(cid: number, settled: readonly TellRead[]): void {
  const wrap = byId('rc-past');
  const list = byId('rc-past-list');
  wrap.hidden = settled.length <= 1;
  const sig = `${cid}#` + settled.map((t) => `${t.at}/${t.name}/${t.state}/${t.ok ? 'y' : 'n'}`).join('|');
  if (sig !== consolePastSig) {
    consolePastSig = sig;
    consolePastRefs = [];
    list.textContent = '';
    settled.forEach((t, i) => {
      const li = el('li');
      const btn = el('button', 'btn sm rcpastbtn');
      btn.type = 'button';
      btn.textContent = `${t.name} · ${tellMark(t).label} · ${fmtStamp(t.at)}`;
      btn.addEventListener('click', () => {
        consoleIdx = i;
        store.scheduleRender();
      });
      li.appendChild(btn);
      list.appendChild(li);
      consolePastRefs.push({ idx: i, btn });
    });
  }
  for (const ref of consolePastRefs) {
    if (ref.idx === consoleIdx) ref.btn.setAttribute('aria-pressed', 'true');
    else ref.btn.removeAttribute('aria-pressed');
  }
}

/* ------------------------------------------------------------- the stop
   The operator's hand on one agent's SESSION: the daemon's `/stop` proxy ends
   the session the hub serves for that guest CID, the hub delivers its stop and
   answers with its own view. It is never a single click — the affordance arms,
   a confirm line states what will happen (the session ends; the guest stays
   up, which is the hub's own contract), and the press that commits names the
   session it stops. Leaving the card disarms it. The result is the hub's own
   words: the ended session's call count, its reason a session still stands, or
   whoever refused, verbatim. */

/** The mark each ending wears, in the register's own language: the ok roundel
    for a session the hub ended, amber for one that still stands (a fact, not a
    failure), a crit mark for a refusal, amber for a node or hub that cannot
    answer. */
const STOP_MARK: Readonly<Record<StopState, { cls: string; label: string }>> = {
  stopping: { cls: 'mk warn', label: 'stopping' },
  ended: { cls: 'mk ok', label: 'the session ended' },
  standing: { cls: 'mk warn', label: 'the session still stands' },
  unserved: { cls: 'mk crit', label: 'no such agent' },
  'not-connected': { cls: 'mk crit', label: 'not connected' },
  off: { cls: 'mk warn', label: 'hub admin off' },
  unreachable: { cls: 'mk warn', label: 'unreachable' },
  refused: { cls: 'mk crit', label: 'refused' },
};

/** A stop's own line: the agent, then the hub's verdict or the status that
    names the refusal. */
function stopReading(read: StopRead): string {
  if (read.state === 'stopping') return `cid ${read.cid} · ending the session …`;
  if (read.state === 'ended') return `cid ${read.cid} · the hub ended the session`;
  if (read.state === 'standing') return `cid ${read.cid} · the hub could not end it`;
  return read.status == null ? `cid ${read.cid} · the node did not answer` : `cid ${read.cid} · HTTP ${read.status}`;
}

/** Render the stop affordance and its result for the console's agent. The arm
    control is offered only while the hub lists the agent as connected — the
    session to end is exactly that connection, and a CID the hub does not list
    can only answer 409; the row says so rather than offering a button that
    cannot work. */
function renderStop(row: HubJoinRow<RoleAgent>, cid: number): void {
  const block = byId('rc-stop');
  block.hidden = false;
  byId('rc-stop-who').textContent = `cid ${cid}`;
  const connected = row.connected != null;
  if (!connected) stopArmed = false; // no session to end; an armed press is meaningless

  const arm = byId<HTMLButtonElement>('rc-stop-arm');
  const note = byId('rc-stop-note');
  const confirm = byId('rc-stop-confirm');
  arm.hidden = !connected;
  note.hidden = connected;
  note.textContent = connected
    ? ''
    : 'The hub does not list this agent as connected, so there is no session to end — a stop would answer 409.';
  const armed = connected && stopArmed;
  confirm.hidden = !armed;
  if (armed) {
    const go = byId<HTMLButtonElement>('rc-stop-go');
    /* the confirm line states what will happen; the button names the session
       it stops, never a bare "OK" */
    byId('rc-stop-line').textContent =
      `Ending the session on cid ${cid}: the agent says bye and the hub closes the session; ` +
      `the guest VM itself stays up.`;
    go.textContent = `Stop the session on cid ${cid}`;
    go.setAttribute(
      'aria-label',
      `Stop the session on cid ${cid} — this ends the session; the guest stays up`,
    );
  }
  renderStopResult(cid);
}

/** The stop's result box: the hub's own answer, kept as it came. */
function renderStopResult(cid: number): void {
  const read = store.lastStop;
  const body = byId('rc-stop-result');
  if (read == null || read.cid !== cid) {
    body.hidden = true;
    return;
  }
  body.hidden = false;
  const mk = STOP_MARK[read.state];
  const mark = byId('rc-stop-mark');
  mark.className = mk.cls;
  mark.textContent = mk.label;
  byId('rc-stop-meta').textContent = stopReading(read);

  /* the hub's own reason a session still stands, or whoever refused — shown
     verbatim; an ended session carries no reason to show */
  const err = byId('rc-stop-err');
  const ended = read.state === 'ended';
  err.hidden = ended || read.state === 'stopping';
  err.textContent = ended || read.state === 'stopping' ? '' : read.detail;

  /* the ended reading: the hub's own verdict, in the register's words — the
     session ended, and the call count it ended with */
  const text = byId<HTMLElement>('rc-stop-text');
  text.hidden = !ended;
  text.textContent = ended
    ? `the session ended · ${fmtNum(read.calls ?? 0)} ${read.calls === 1 ? 'call' : 'calls'}`
    : '';

  /* a screen reader hears each stop once, and only settled ones — the pending
     state is the press it already made */
  if (read.state !== 'stopping') {
    const key = `${read.cid}\u0000${read.at}\u0000${read.state}`;
    if (key !== stopAnnounced) {
      stopAnnounced = key;
      announce(
        read.state === 'ended'
          ? `The session on the agent at cid ${read.cid} ended after ${fmtNum(read.calls ?? 0)} calls.`
          : read.state === 'standing'
            ? `The hub could not end the session on the agent at cid ${read.cid}: ${read.detail}`
            : `The stop on the agent at cid ${read.cid} was ${mk.label}: ${read.detail}`,
      );
    }
  }
}

/** Arm the stop: reveal the confirm line and move focus to the press that
    commits, so a keyboard owner is never left on a press that did nothing. */
function armStop(): void {
  const reason = byId<HTMLInputElement>('rc-stop-reason');
  if (reason.value !== '') reason.value = ''; // a fresh arm starts from an empty reason
  byId('rc-stop-form-err').textContent = '';
  stopArmed = true;
  store.scheduleRender();
  queueMicrotask(() => byId<HTMLButtonElement>('rc-stop-go').focus());
}

/** Disarm the stop: leave the session as it stands. */
function disarmStop(): void {
  stopArmed = false;
  store.scheduleRender();
  queueMicrotask(() => byId<HTMLButtonElement>('rc-stop-arm').focus());
}

/** Commit the armed stop: the reason is read from the box (named at the field
    when it is past the hub's byte bound), then posted. */
function sendStop(): void {
  const cid = consoleCid;
  if (cid == null) return;
  const parsed = parseStopReason(byId<HTMLInputElement>('rc-stop-reason').value);
  if ('error' in parsed) {
    byId('rc-stop-form-err').textContent = parsed.error;
    return;
  }
  byId('rc-stop-form-err').textContent = '';
  stopArmed = false;
  stopAnnounced = '';
  void store.stopSession(cid, parsed.reason);
}

/** The roster's own wiring: a row opens its run's entry, the register's rule;
    a row's Drive button opens the tool console on that agent. A hub-only row
    has no entry to open, so it stays inert but is still drivable. */
export function initRoster(): void {
  byId('roster-body').addEventListener('click', (e) => {
    const target = e.target as HTMLElement | null;
    const drive = target?.closest<HTMLElement>('[data-drive]');
    if (drive) {
      /* the button lives inside a `data-go` row (the row opens the run), and
         the document's own row navigator would open it too: this click is the
         console's, not the row's, so it stops here */
      e.stopPropagation();
      openConsole(Number(drive.getAttribute('data-drive')));
      return;
    }
    const row = target?.closest<HTMLElement>('tr[data-go]');
    if (!row) return;
    const dest = row.getAttribute('data-go');
    if (dest) location.hash = dest;
  });
  byId<HTMLFormElement>('rc-form').addEventListener('submit', sendConsole);
  byId('rc-close').addEventListener('click', closeConsole);
  byId('rc-stop-arm').addEventListener('click', armStop);
  byId('rc-stop-cancel').addEventListener('click', disarmStop);
  byId('rc-stop-go').addEventListener('click', sendStop);
}
