/* The store — the single copy of daemon state. Every screen renders from
   this; no view keeps its own divergent copy. Runs arrive whole from the
   daemon; serial text is append-only per VM; activity is derived from poll
   deltas. Rendering is coalesced through a microtask: the paint path never
   depends on animation frames, so hidden tabs still paint what they have. */

import { getMachine, getRun, listRuns } from './api';
import { ActivityTracker, vmKey } from './models/activity';
import type { Machine } from './models/machine';
import type { Run } from './models/run';
import { seqOf } from './models/run';
import type { SerialCache } from './serial';
import { linesOf } from './serial';
import { toast } from './dom';

export type ViewName = 'register' | 'run' | 'machine' | 'map' | 'view3d';
export type Theme = 'system' | 'light' | 'dark';
export type Health = 'live' | 'stale' | 'down';

export interface Route {
  name: ViewName;
  param: string | null;
}

const TICK_MS = 1000; // detail cadence; the list polls every 2 ticks
const MACHINE_EVERY = 15; // ticks between machine polls
const LIVE_WINDOW_MS = 4000; // a success older than this reads "stale"

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
  route: Route = { name: 'register', param: null };

  /* derived caches */
  readonly serial = new Map<string, SerialCache>(); // "r3/0" -> lines
  readonly activity = new ActivityTracker();

  /* health */
  private lastOkAt = 0;
  private lastErrAt = 0;

  /* polling internals */
  private busy = false;
  private ticks = 0;
  private timer: number | null = null;
  private renderQueued = false;
  private readonly renderers: Array<() => void> = [];

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
      for (const render of this.renderers) render();
    });
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
    } else if (route.name === 'map' || route.name === 'view3d') {
      /* The map and 3D view pulse with serial activity, which only the
         single-run GET carries: poll every open run's detail. */
      for (const run of this.runs) {
        if (run.state === 'starting' || run.state === 'running') jobs.push(this.pollDetail(run.id, true));
      }
    }
    this.ticks++;
    await Promise.all(jobs);
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
      /* forget caches of runs that left the register */
      const alive = new Set(runs.map((r) => r.id));
      for (const id of [...this.details.keys()]) {
        if (!alive.has(id)) this.details.delete(id);
      }
      for (const key of [...this.serial.keys()]) {
        if (!alive.has(key.slice(0, key.indexOf('/')))) this.serial.delete(key);
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
    const old = this.details.get(run.id);
    const now = Date.now();
    run.vms.forEach((v, i) => {
      if (v.serial == null) {
        const kept = old?.vms[i]?.serial;
        if (kept != null) v.serial = kept;
      }
      const key = vmKey(run.id, v.id);
      this.activity.observe(key, linesOf(this.serial, key, v.serial).length, now);
    });
    this.details.set(run.id, run);
  }

  /** Lines for a VM of the open run, from the append-only cache. */
  linesFor(runId: string, vmId: number, serialRaw: string | undefined): string[] {
    return linesOf(this.serial, vmKey(runId, vmId), serialRaw);
  }
}

export const store = new Store();
