/* The store — the single copy of daemon state. Every screen renders from
   this; no view keeps its own divergent copy. Runs arrive whole from the
   daemon; serial text is append-only per VM; activity is derived from poll
   deltas. Rendering is coalesced through a microtask: the paint path never
   depends on animation frames, so hidden tabs still paint what they have. */

import { getMachine, getRun, listRuns } from './api';
import { ActivityTracker, vmKey } from './models/activity';
import type { RunSlice } from './models/history';
import { SessionHistory } from './models/history';
import type { Machine } from './models/machine';
import type { Run, RunState, VmState } from './models/run';
import { isOpen, seqOf } from './models/run';
import type { SerialCache } from './serial';
import { linesOf, serialDelta } from './serial';
import { toast } from './dom';

export type ViewName = 'register' | 'run' | 'machine' | 'map' | 'view3d' | 'compare' | 'telemetry';
export type Theme = 'system' | 'light' | 'dark';
export type Health = 'live' | 'stale' | 'down';
/** The event stream's own liveness — shown in the colophon, never feeds daemon health. */
export type StreamState = 'connecting' | 'live' | 'reconnecting';

declare global {
  interface Window {
    /** Verification hook: a harness may set this before boot to slow the
        poll clock, proving the event stream — not the polls — carries
        liveness. Production never sets it. */
    __PLAZA_TICK_MS__?: number;
    /** Read-only counters for the verification harness; the app never reads them. */
    __PLAZA_DIAG__?: {
      readonly renders: number;
      readonly wire: { readonly run: number; readonly vm: number; readonly serial: number };
      readonly stream: StreamState;
    };
  }
}

/** The pair a #compare route names: run a's vm avm against run b's vm bvm. */
export interface CompareTarget {
  a: string;
  avm: number;
  b: string;
  bvm: number;
}

export interface Route {
  name: ViewName;
  param: string | null;
  compare: CompareTarget | null;
}

const TICK_MS =
  typeof window !== 'undefined' && typeof window.__PLAZA_TICK_MS__ === 'number' && window.__PLAZA_TICK_MS__ > 0
    ? window.__PLAZA_TICK_MS__
    : 1000; // detail cadence; the list polls every 2 ticks
const MACHINE_EVERY = 15; // ticks between machine polls
const LIVE_WINDOW_MS = 4000; // a success older than this reads "stale"
const EVENT_REFRESH_MS = 150; // a burst of run events collapses into one list GET
const LIVE_RENDER_MS = 100; // the live lane's bound: at most this often under a serial burst
const VM_PUSH_MIN_MS = 1000; // a VM's sparkline ring gains at most one point per this window

export class Store {
  /* daemon state */
  machine: Machine | null = null;
  machineError: string | null = null;
  runs: Run[] = []; // newest first
  runsError: string | null = null;
  /** Every run fetched with serial, by id (map/3D poll each open run). */
  readonly details = new Map<string, Run>();
  detailError: string | null = null;

  /* view state */
  detailId: string | null = null;
  vm = 0; // selected VM index within the open run
  follow = true; // transcript tail-follow
  theme: Theme = 'system';
  route: Route = { name: 'register', param: null, compare: null };
  /** The telemetry view's run pick; null is the whole fleet. */
  telemetryRun: string | null = null;

  /* derived caches */
  readonly serial = new Map<string, SerialCache>(); // "r3/0" -> lines
  readonly activity = new ActivityTracker();
  readonly history = new SessionHistory();

  /* health */
  private lastOkAt = 0;
  private lastErrAt = 0;

  /* the live wire (GET /api/v0/events): its own liveness and tallies —
     informational only; daemon health stays with the polls */
  stream: StreamState = 'connecting';
  readonly wire = { run: 0, vm: 0, serial: 0 };
  /** Render passes flushed; the harness asserts a burst cannot storm the paint path. */
  renders = 0;

  /* polling internals */
  private busy = false;
  private ticks = 0;
  private timer: number | null = null;
  private renderQueued = false;
  private readonly renderers: Array<() => void> = [];

  /* live-wire internals */
  private liveRenderAt = 0;
  private liveRenderTimer: number | null = null;
  private listRefreshTimer: number | null = null;
  private readonly detailRefreshPending = new Set<string>();
  private readonly vmPushAt = new Map<string, number>();

  get health(): Health {
    if (this.lastOkAt === 0) return this.lastErrAt > 0 ? 'down' : 'stale';
    if (this.lastErrAt > this.lastOkAt && Date.now() - this.lastOkAt > LIVE_WINDOW_MS) return 'down';
    return Date.now() - this.lastOkAt <= LIVE_WINDOW_MS ? 'live' : 'stale';
  }

  /* ------------------------------------------------------------ rendering */
  onRender(render: () => void): void {
    this.renderers.push(render);
  }

  scheduleRender(): void {
    if (this.renderQueued) return;
    this.renderQueued = true;
    queueMicrotask(() => {
      this.renderQueued = false;
      this.renders++;
      for (const render of this.renderers) render();
    });
  }

  /** The live lane, for the event stream's serial frames: immediate while
      quiet, at most one paint per LIVE_RENDER_MS under a burst, and always
      a trailing paint so the last frame of a burst is never lost. A guest
      spewing serial can never storm the renderers. */
  private scheduleLiveRender(): void {
    if (this.liveRenderTimer != null) return; // a trailing paint is already queued
    const since = Date.now() - this.liveRenderAt;
    if (since >= LIVE_RENDER_MS) {
      this.liveRenderAt = Date.now();
      this.scheduleRender();
    } else {
      this.liveRenderTimer = setTimeout(() => {
        this.liveRenderTimer = null;
        this.liveRenderAt = Date.now();
        this.scheduleRender();
      }, LIVE_RENDER_MS - since);
    }
  }

  /* -------------------------------------------------------------- polling
     The interval pauses while the tab is hidden — except that showing a view
     always fetches once (ensurePolled), so a view opened in a hidden tab still
     paints. Visibility flips back to visible restart the clock. */
  startPolling(): void {
    if (this.timer != null) return;
    void this.tick();
    this.timer = setInterval(() => {
      if (!document.hidden) void this.tick();
    }, TICK_MS);
    document.addEventListener('visibilitychange', () => {
      if (!document.hidden) void this.tick();
    });
  }

  /** The one fetch every view gets even while hidden. */
  ensurePolled(): void {
    void this.pollFor(this.route, true);
  }

  private async tick(): Promise<void> {
    if (this.busy) return;
    this.busy = true;
    try {
      await this.pollFor(this.route, false);
    } finally {
      this.busy = false;
    }
  }

  private async pollFor(route: Route, first: boolean): Promise<void> {
    const jobs: Array<Promise<void>> = [];
    const wantList = first || this.ticks % 2 === 0 || this.runsError != null || this.runs.length === 0;
    if (wantList) jobs.push(this.pollList());
    if (first || this.ticks % MACHINE_EVERY === 0 || route.name === 'machine') jobs.push(this.pollMachine());

    if (route.name === 'run' && this.detailId) {
      jobs.push(this.pollDetail(this.detailId));
    } else if (route.name === 'compare' && route.compare) {
      /* both sides of the pair, with serial; quiet — a run that left the
         register shows in its pane, never a redirect */
      jobs.push(this.pollDetail(route.compare.a, true));
      if (route.compare.b !== route.compare.a) jobs.push(this.pollDetail(route.compare.b, true));
    } else if (route.name === 'map' || route.name === 'view3d' || route.name === 'telemetry') {
      /* The map, 3D view and telemetry read serial activity, which only the
         single-run GET carries: poll every open run's detail. */
      for (const run of this.runs) {
        if (run.state === 'starting' || run.state === 'running') jobs.push(this.pollDetail(run.id, true));
      }
    }
    this.ticks++;
    const okBefore = this.lastOkAt;
    await Promise.all(jobs);
    /* one fleet sample per poll that actually observed something — a poll
       that only failed writes nothing, so unobserved time stays un-inked */
    if (this.lastOkAt !== okBefore) this.sampleHistory();
  }

  private noteOk(): void {
    this.lastOkAt = Date.now();
  }

  private noteErr(): void {
    this.lastErrAt = Date.now();
  }

  private async pollList(): Promise<void> {
    try {
      const body = await listRuns();
      const runs = (body.runs ?? []).slice();
      runs.sort((a, b) => seqOf(b.id) - seqOf(a.id));
      /* identity is (id, created_at): a restarted daemon hands the same id
         out again — a changed created_at is a different run wearing it */
      for (const run of runs) {
        const prev = this.runs.find((r) => r.id === run.id);
        if (prev && prev.created_at !== run.created_at) this.forgetRun(run.id);
        const detail = this.details.get(run.id);
        if (detail && detail.created_at !== run.created_at) this.forgetRun(run.id);
      }
      /* forget caches of runs that left the register */
      const alive = new Set(runs.map((r) => r.id));
      for (const id of new Set([...this.runs.map((r) => r.id), ...this.details.keys()])) {
        if (!alive.has(id)) this.forgetRun(id);
      }
      this.runs = runs;
      this.runsError = null;
      this.noteOk();
    } catch (err) {
      this.runsError = err instanceof Error ? err.message : String(err);
      this.noteErr();
    }
    this.scheduleRender();
  }

  private async pollMachine(): Promise<void> {
    try {
      this.machine = await getMachine();
      this.machineError = null;
      this.noteOk();
    } catch (err) {
      this.machineError = err instanceof Error ? err.message : String(err);
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** One fleet observation, derived from what the client already holds: the
      register (live VMs, committed RAM) and the activity tracker (chatter). */
  private sampleHistory(): void {
    let vms = 0;
    let ramMib = 0;
    let lps = 0;
    const runs: Record<string, RunSlice> = {};
    for (const run of this.runs) {
      let live = 0;
      let runLps = 0;
      for (const v of run.vms) {
        if (v.state !== 'running') continue;
        live++;
        /* chatter counts while the VM is alive; an exited VM's EMA would
           otherwise read as fresh throughput forever after the run closes */
        runLps += this.activity.get(vmKey(run.id, v.id));
      }
      runs[run.id] = { vms: live, ramMib: live * run.memory_mib, lps: runLps };
      vms += live;
      ramMib += live * run.memory_mib;
      lps += runLps;
    }
    this.history.push({ at: Date.now(), vms, ramMib, lps, runs });
  }

  /** The run the run-detail view is open on, with serial when fetched. */
  get openRun(): Run | null {
    return this.detailId ? (this.details.get(this.detailId) ?? null) : null;
  }

  /** Fetch one run with serial. `quiet` polls (map/3D) skip the 404 redirect. */
  async pollDetail(id: string, quiet = false): Promise<void> {
    try {
      const run = await getRun(id);
      this.mergeDetail(run);
      if (this.detailId === id) this.detailError = null;
      this.noteOk();
    } catch (err) {
      const status = err && typeof err === 'object' && 'status' in err ? err.status : null;
      if (status === 404 && !quiet && this.detailId === id) {
        toast(`Run ${id} is not on the register`);
        this.details.delete(id);
        this.detailError = null;
        this.detailId = null;
        if (location.hash !== '#register') location.hash = '#register';
      } else if (this.detailId === id) {
        this.detailError = err instanceof Error ? err.message : String(err);
      }
      this.noteErr();
    }
    this.scheduleRender();
  }

  /** Merge a freshly fetched run: keep serial a response did not carry
      (DELETE answers without it), feed the activity tracker. */
  mergeDetail(run: Run): void {
    let old = this.details.get(run.id);
    if (old && old.created_at !== run.created_at) {
      /* the daemon began this id again: the old run is not this run */
      this.forgetRun(run.id);
      old = undefined;
    }
    const now = Date.now();
    run.vms.forEach((v, i) => {
      const kept = old?.vms[i]?.serial;
      if (v.serial == null) {
        if (kept != null) v.serial = kept;
      } else if (kept != null && kept.length > v.serial.length && kept.startsWith(v.serial)) {
        /* the event stream already carried bytes past this response's
           snapshot — the capture only grows, so the longer copy is newer */
        v.serial = kept;
      }
      const key = vmKey(run.id, v.id);
      const lps = this.activity.observe(key, linesOf(this.serial, key, v.serial).length, now);
      this.history.pushVm(key, lps);
      this.vmPushAt.set(key, now);
    });
    this.details.set(run.id, run);
  }

  /* ---------------------------------------------------------- the live wire
     The SSE consumer (events.ts) drives these. Every event lands in the
     same caches the polls fill, so every view updates through the one
     render path — and the polls reconcile anything the stream missed (the
     bus has no backlog; a reconnect's lost window heals on the GETs that
     each (re)open triggers). */

  setStreamState(state: StreamState): void {
    if (this.stream === state) return;
    this.stream = state;
    this.scheduleRender();
  }

  /** A run changed state (or was registered, as `starting`). Patch the
      copies the views read; the register's full row (shape, closed time)
      comes from a debounced list GET — a burst of transitions collapses
      into one fetch. A closed run's `ended_at` comes from its detail.
      An event that would wake an ENDED run is never believed: ids restart
      with the daemon, so that is a new run wearing the id — the register
      is asked at once (its answer carries every run's created_at). */
  applyRunEvent(id: string, state: RunState): void {
    this.wire.run++;
    const listed = this.runs.find((r) => r.id === id);
    const detail = this.details.get(id);
    const known = detail ?? listed;
    if (known && !isOpen(known) && (state === 'starting' || state === 'running')) {
      this.queueListRefresh(true);
      return;
    }
    if (listed) listed.state = state;
    if (detail) detail.state = state;
    this.scheduleRender();
    this.queueListRefresh();
    if (state !== 'starting' && state !== 'running') this.queueDetailRefresh(id);
  }

  /** A VM changed state. The mark, the tallies and the stamps all read the
      store's copies — patch both (the register's list row and the detail).
      A `running` event for an ended run is the restarted-id case above. */
  applyVmEvent(runId: string, vmId: number, state: VmState): void {
    this.wire.vm++;
    const listed = this.runs.find((r) => r.id === runId);
    const detail = this.details.get(runId);
    const known = detail ?? listed;
    if (known && !isOpen(known) && state === 'running') {
      this.queueListRefresh(true);
      return;
    }
    for (const run of [listed, detail]) {
      const vm = run?.vms.find((v) => v.id === vmId);
      if (vm) vm.state = state;
    }
    this.scheduleRender();
  }

  /** A VM's serial capture grew. `offset` is the capture byte the text
      starts at; the merge is offset-idempotent — append the new stretch,
      clip a re-delivery, and let a detail GET reconcile a gap (a missed
      frame or poll; rare by construction). The delta feeds the line cache
      and the activity EMA exactly as a poll merge does, so the transcript,
      the lines/s figures, the map's pulse and the 3D glow all go live
      between polls. */
  applySerialEvent(runId: string, vmId: number, offset: number, text: string): void {
    this.wire.serial++;
    const run = this.details.get(runId);
    const vm = run?.vms.find((v) => v.id === vmId);
    if (!vm || vm.serial == null) {
      /* no baseline to offset against — the detail GET carries the capture */
      this.queueDetailRefresh(runId);
      return;
    }
    const key = vmKey(runId, vmId);
    linesOf(this.serial, key, vm.serial); // sync the cache before the offset math
    const have = this.serial.get(key)?.bytes ?? 0;
    const delta = serialDelta(have, offset, text);
    if (delta.kind === 'gap') {
      this.queueDetailRefresh(runId);
      return;
    }
    if (delta.kind === 'dup') return;
    vm.serial += delta.text;
    const now = Date.now();
    const lps = this.activity.observe(key, linesOf(this.serial, key, vm.serial).length, now);
    /* the sparkline ring keeps its one-observation-per-poll timebase:
       between polls it gains at most one point per VM_PUSH_MIN_MS window */
    if (now - (this.vmPushAt.get(key) ?? 0) >= VM_PUSH_MIN_MS) {
      this.vmPushAt.set(key, now);
      this.history.pushVm(key, lps);
    }
    this.scheduleLiveRender();
  }

  /** One list GET shortly after run events land; a burst collapses into it.
      An identity question (an ended run supposedly waking) asks NOW — the
      register's answer carries every run's created_at. */
  private queueListRefresh(immediate = false): void {
    if (immediate) {
      if (this.listRefreshTimer != null) {
        clearTimeout(this.listRefreshTimer);
        this.listRefreshTimer = null;
      }
      void this.pollList();
      return;
    }
    if (this.listRefreshTimer != null) return;
    this.listRefreshTimer = setTimeout(() => {
      this.listRefreshTimer = null;
      void this.pollList();
    }, EVENT_REFRESH_MS);
  }

  /** Forget everything held for a run. Run ids RESTART with the daemon:
      after a restart the first run is r1 again — a different run wearing
      the same id. Identity is (id, created_at); when it changes, the old
      run's captures, activity and history must not bleed into the new one. */
  private forgetRun(id: string): void {
    this.details.delete(id);
    const prefix = id + '/';
    for (const key of [...this.serial.keys()]) {
      if (key.startsWith(prefix)) {
        this.serial.delete(key);
        this.vmPushAt.delete(key);
      }
    }
    this.activity.dropRun(id);
    this.history.dropRun(id);
  }

  /** A detail GET reconciles what an event could not carry (a serial gap,
      a run the register has not listed yet, a closed run's ended_at).
      One in flight per run. */
  private queueDetailRefresh(id: string): void {
    if (this.detailRefreshPending.has(id)) return;
    this.detailRefreshPending.add(id);
    void this.pollDetail(id, true).finally(() => this.detailRefreshPending.delete(id));
  }

  /** Lines for a VM of the open run, from the append-only cache. */
  linesFor(runId: string, vmId: number, serialRaw: string | undefined): string[] {
    return linesOf(this.serial, vmKey(runId, vmId), serialRaw);
  }
}

export const store = new Store();
