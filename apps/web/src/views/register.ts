/* The register: machine panel and fleet tallies up top, the spawn form,
   then every run newest-first. Entries rebuild only when the register's shape
   changes; durations tick in place so focus is never stolen. */

import { createRun } from '../api';
import { announce, byId, el, toast } from '../dom';
import { fmtDur, fmtNum, fmtStamp, fmtTime, pad } from '../format';
import type { Run } from '../models/run';
import { hasVsock, isOpen, modeText, roleOf, seqOf } from '../models/run';
import { store } from '../store';
import { runStateMk } from './chrome';

interface RunRow {
  run: Run;
  dur: Text;
}

let runsSig: string | null = null;
let runRows: Record<string, RunRow> = {};

/* ------------------------------------------------------------ rendering */
export function renderRegister(): void {
  renderNotice();
  renderMachinePanel();
  renderTallies();
  renderRuns();
}

function renderNotice(): void {
  const notice = byId('notice');
  if (store.runsError != null) {
    notice.hidden = false;
    byId('notice-text').textContent =
      `The node daemon is not answering at ${location.origin} (${store.runsError}). ` +
      'Retrying every two seconds; the page catches up on its own.';
  } else {
    notice.hidden = true;
  }
}

function renderMachinePanel(): void {
  const m = store.machine;
  byId('m-name').textContent = m?.name ?? (store.machineError ? 'unreachable' : '—');
  byId('m-class').textContent = m ? `${m.class} · irqchip ${m.irqchip}` : '—';
  byId('m-arch').textContent = m ? `${m.arch} · ${m.backend}` : '—';
  byId('m-cpus').textContent = m ? fmtNum(m.cpus) : '—';
  byId('m-mem').textContent = m ? fmtNum(m.memory_mib) + ' MiB' : '—';
  byId('m-vsock').textContent = m?.vsock_mode ?? '—';
  byId('m-uptime').textContent = m ? fmtDur(Date.now() - m.started_at_ms) : '—';
}

function renderTallies(): void {
  let liveRuns = 0, vmsRunning = 0, vcpus = 0, mem = 0;
  for (const run of store.runs) {
    if (!isOpen(run)) continue;
    liveRuns++;
    for (const vm of run.vms) {
      if (vm.state !== 'running') continue;
      vmsRunning++;
      vcpus += run.vcpus;
      mem += run.memory_mib;
    }
  }
  byId('t-runs').textContent = fmtNum(liveRuns);
  byId('t-vms').textContent = fmtNum(vmsRunning);
  byId('t-vcpus').textContent = fmtNum(vcpus);
  byId('t-mem').textContent = fmtNum(mem);

  const live = byId('runs-live');
  live.hidden = liveRuns === 0;
  live.textContent = `${liveRuns} live`;
}

function renderRuns(): void {
  const count = byId('reg-count');
  count.textContent =
    store.runsError != null
      ? 'last known'
      : fmtNum(store.runs.length) + (store.runs.length === 1 ? ' entry' : ' entries');
  byId('runs-empty').hidden = !(store.runsError == null && store.runs.length === 0);

  const sig = store.runs
    .map((r) => `${r.id}:${r.state}:${r.ended_at ?? 0}:${r.vms.length}:${hasVsock(r) ? 1 : 0}:${r.vms.filter((v) => roleOf(v) != null).length}`)
    .join('|');
  const list = byId('runs-list');
  if (sig !== runsSig) {
    runsSig = sig;
    runRows = {};
    list.textContent = '';
    for (const r of store.runs) {
      const seq = seqOf(r.id);
      /* a role fleet is not an ordinary run: its VM rows carry roles, so the
         entry says so in the margin — an ink mark, never a tinted pill, and
         only when the register actually names one */
      const roles = r.vms.filter((v) => roleOf(v) != null).length;
      const b = el('b', null, `Run ${r.id} — ${r.vms.length} VMs × ${r.vcpus} vCPUs, ${fmtNum(r.memory_mib)} MiB each`);
      const sub = el('span');
      sub.appendChild(document.createTextNode(`${modeText(r)} · opened ${fmtStamp(r.created_at)}`));
      const dur = document.createTextNode('');
      sub.appendChild(el('span')).appendChild(dur);
      const matter = el('span', 'matter');
      matter.appendChild(b);
      matter.appendChild(sub);
      const btn = el('button', 'entry');
      btn.type = 'button';
      btn.setAttribute('data-go', `#run/${r.id}`);
      btn.setAttribute('aria-label',
        `Open run ${r.id}, ${r.state}${hasVsock(r) ? ', vsock' : ''}${roles > 0 ? `, role fleet of ${roles}` : ''}`);
      btn.appendChild(el('span', 'no', pad(seq, 4)));
      btn.appendChild(matter);
      btn.appendChild(el('span', 'due', fmtStamp(r.created_at)));
      const auth = el('span', 'auth');
      if (hasVsock(r)) auth.appendChild(el('span', 'mk vsock', 'vsock'));
      if (roles > 0) auth.appendChild(el('span', 'mk', `fleet · ${fmtNum(roles)} ${roles === 1 ? 'role' : 'roles'}`));
      auth.appendChild(runStateMk(r.state));
      btn.appendChild(auth);
      list.appendChild(btn);
      runRows[r.id] = { run: r, dur };
    }
  }
  const now = Date.now();
  for (const id of Object.keys(runRows)) {
    const row = runRows[id];
    if (!row) continue;
    const r = row.run;
    row.dur.nodeValue =
      r.ended_at != null
        ? ` · closed ${fmtTime(r.ended_at)} after ${fmtDur(r.ended_at - r.created_at)}`
        : ` · ${fmtDur(now - r.created_at)} so far`;
  }
}

/* ------------------------------------------------------------ spawn form */
/* Quick entries: one press fills the manual form with a known-good shape
   (the user sees exactly what will boot and can adjust it); spawning still
   goes through the form's own validation and POST path. */
interface Preset {
  n: number;
  vcpus: number;
  mem: number;
  beats: number | null;
  vsock: number | null;
}

const PRESETS: Readonly<Record<string, Preset>> = {
  /* the full session demo: two guests on vsock cids 10–11; the orchestrator
     stand-in speaks hello/welcome/echo/bye and the run closes itself */
  vsock: { n: 2, vcpus: 2, mem: 512, beats: null, vsock: 10 },
  /* the fleet demo: eight guests heartbeat a dozen times, then power off */
  churn: { n: 8, vcpus: 2, mem: 512, beats: 12, vsock: null },
  /* the validated scale scenario: 50 × 128 MiB (≈ 6.4 GiB guest RAM), 12
     beats, all fifty exit cleanly inside a minute */
  scale: { n: 50, vcpus: 1, mem: 128, beats: 12, vsock: null },
};

function readInt(id: string): number | null {
  const raw = byId<HTMLInputElement>(id).value.trim();
  if (raw === '') return null;
  return Number(raw);
}

export function initRegister(): void {
  document.querySelectorAll<HTMLElement>('[data-preset]').forEach((btn) => {
    btn.addEventListener('click', () => {
      const p = PRESETS[btn.dataset['preset'] ?? ''];
      if (!p) return;
      byId<HTMLInputElement>('f-n').value = String(p.n);
      byId<HTMLInputElement>('f-vcpus').value = String(p.vcpus);
      byId<HTMLInputElement>('f-mem').value = String(p.mem);
      byId<HTMLInputElement>('f-beats').value = p.beats == null ? '' : String(p.beats);
      byId<HTMLInputElement>('f-vsock').value = p.vsock == null ? '' : String(p.vsock);
      byId('spawn-err').textContent = '';
      byId<HTMLButtonElement>('spawn-btn').focus();
      const shape = `${p.n} ${p.n === 1 ? 'VM' : 'VMs'} × ${p.vcpus} vCPUs · ${fmtNum(p.mem)} MiB each` +
        (p.beats != null ? ` · ${p.beats} beats` : ' · steady') + (p.vsock != null ? ` · vsock cid ${p.vsock}` : '');
      toast(`Form filled: ${shape} — press Spawn the run to boot`);
      announce(`Form filled for ${p.n} VMs. Review the fields, then press Spawn the run.`);
    });
  });

  byId('spawn').addEventListener('submit', (e) => {
    e.preventDefault();
    const errEl = byId('spawn-err');
    errEl.textContent = '';
    const n = readInt('f-n');
    const vcpus = readInt('f-vcpus');
    const mem = readInt('f-mem');
    const beats = readInt('f-beats');
    const vsock = readInt('f-vsock');
    const bad = (msg: string): void => { errEl.textContent = msg; };
    if (n == null || !Number.isInteger(n) || n < 1 || n > 64) return bad('n must be a whole number from 1 to 64.');
    if (vcpus == null || !Number.isInteger(vcpus) || vcpus < 1 || vcpus > 64) return bad('vCPUs must be a whole number from 1 to 64.');
    if (mem == null || !Number.isInteger(mem) || mem < 128) return bad('Memory must be 128 MiB or more.');
    if (beats != null && (!Number.isInteger(beats) || beats < 1)) return bad('Beats must be a whole number of 1 or more, or empty for steady mode.');
    if (vsock != null && (!Number.isInteger(vsock) || vsock < 3)) return bad('vsock CID must be a whole number of 3 or more (1 and 2 are reserved), or empty for no vsock.');
    /* the daemon's own bounds, mirrored so the form refuses before the round
       trip (noded CreateRun::spec; the desktop form uses the same wording) */
    const MAX_CID = 4294967294; // vmm::spawner::MAX_GUEST_CID — u32::MAX − 1
    if (vsock != null && vsock > MAX_CID) {
      return bad('vsock_cid must be at most 4294967294: the kernel refuses VMADDR_CID_ANY and above.');
    }
    if (vsock != null && vsock + n - 1 > MAX_CID) {
      return bad('vsock_cid and n together run past the last guest CID.');
    }

    const body: { n: number; vcpus: number; memory_mib: number; beats?: number; vsock_cid?: number } =
      { n, vcpus, memory_mib: mem };
    if (beats != null) body.beats = beats;
    if (vsock != null) body.vsock_cid = vsock;

    const btn = byId<HTMLButtonElement>('spawn-btn');
    btn.disabled = true;
    btn.textContent = 'Entering…';
    createRun(body)
      .then((run) => {
        toast(`Run ${run.id} entered — ${n} ${n === 1 ? 'VM' : 'VMs'} spawning`);
        store.mergeDetail(run);
        store.detailId = run.id;
        store.vm = 0;
        announce(`Run ${run.id} entered.`);
        if (location.hash !== `#run/${run.id}`) location.hash = `#run/${run.id}`;
        else store.scheduleRender();
      })
      .catch((err: unknown) => {
        errEl.textContent = 'The daemon refused: ' + (err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        btn.disabled = false;
        btn.textContent = 'Spawn the run';
      });
  });
}
